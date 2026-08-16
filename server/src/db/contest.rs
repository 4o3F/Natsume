use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, QueryableByName, RunQueryDsl,
    dsl::sql,
    sql_types::{BigInt, Text},
    sqlite::SqliteConnection,
};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    application::contest::{
        AccountFacts, BindingFacts, ContestError, DeviceFacts, DeviceId, DeviceLifecycleAction,
        DeviceLifecycleFacts, DeviceLifecycleOutcome, DeviceState, HardwareIdentityQuality,
        SeatFacts, decide_device_lifecycle,
    },
    audit::{
        self, AuditDetail, AuditEvent, AuditEventId, CorrelationId, DeviceLifecycleAuditResult,
    },
    db::{
        Database,
        schema::{accounts, device_bindings, device_tokens, devices, gateway_certificates, seats},
    },
};

pub(crate) async fn list_seats(database: &Database) -> Result<Vec<SeatFacts>, ContestError> {
    database
        .interact(|connection| {
            seats::table
                .select((seats::seat_id, seats::seat_code))
                .order(seats::seat_id)
                .load::<(String, String)>(connection)
                .map(|rows| {
                    rows.into_iter()
                        .map(|(seat_id, seat_code)| SeatFacts::new(seat_id, seat_code))
                        .collect()
                })
                .map_err(|_| ContestStoreError::ReadFailed)
        })
        .await
        .map_err(|_| ContestStoreError::AcquireFailed)?
        .map_err(ContestError::from)
}

pub(crate) async fn list_accounts(database: &Database) -> Result<Vec<AccountFacts>, ContestError> {
    database
        .interact(|connection| {
            accounts::table
                .select((
                    accounts::account_id,
                    accounts::domjudge_username,
                    sql::<BigInt>("credential_revision"),
                ))
                .order(accounts::account_id)
                .load::<(String, String, i64)>(connection)
                .map(|rows| {
                    rows.into_iter()
                        .map(|(account_id, domjudge_username, credential_revision)| {
                            AccountFacts::new(account_id, domjudge_username, credential_revision)
                        })
                        .collect()
                })
                .map_err(|_| ContestStoreError::ReadFailed)
        })
        .await
        .map_err(|_| ContestStoreError::AcquireFailed)?
        .map_err(ContestError::from)
}

pub(crate) async fn list_devices(database: &Database) -> Result<Vec<DeviceFacts>, ContestError> {
    database
        .interact(|connection| {
            devices::table
                .select((
                    devices::device_pk,
                    devices::state,
                    devices::hardware_identity_quality,
                ))
                .order(devices::device_pk)
                .load::<(String, String, String)>(connection)
                .map_err(|_| ContestStoreError::ReadFailed)?
                .into_iter()
                .map(|(device_id, state, hardware_identity_quality)| {
                    Ok(DeviceFacts::new(
                        device_id,
                        DeviceState::from_persisted(&state)
                            .map_err(|_| ContestStoreError::InvalidPersistedFacts)?,
                        HardwareIdentityQuality::from_persisted(&hardware_identity_quality)
                            .map_err(|_| ContestStoreError::InvalidPersistedFacts)?,
                    ))
                })
                .collect::<Result<Vec<_>, ContestStoreError>>()
        })
        .await
        .map_err(|_| ContestStoreError::AcquireFailed)?
        .map_err(ContestError::from)
}

pub(crate) async fn list_bindings(database: &Database) -> Result<Vec<BindingFacts>, ContestError> {
    database
        .interact(|connection| {
            device_bindings::table
                .select((
                    device_bindings::seat_id,
                    device_bindings::device_pk,
                    sql::<BigInt>("binding_revision"),
                ))
                .order(device_bindings::seat_id)
                .load::<(String, String, i64)>(connection)
                .map(|rows| {
                    rows.into_iter()
                        .map(|(seat_id, device_id, binding_revision)| {
                            BindingFacts::new(seat_id, device_id, binding_revision)
                        })
                        .collect()
                })
                .map_err(|_| ContestStoreError::ReadFailed)
        })
        .await
        .map_err(|_| ContestStoreError::AcquireFailed)?
        .map_err(ContestError::from)
}

pub(crate) async fn apply_device_lifecycle(
    database: &Database,
    device_id: &DeviceId,
    action: DeviceLifecycleAction,
    correlation_id: CorrelationId,
) -> Result<(), ContestError> {
    apply_device_lifecycle_with_audit_id(
        database,
        device_id,
        action,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(ContestError::from)
}

async fn apply_device_lifecycle_with_audit_id(
    database: &Database,
    device_id: &DeviceId,
    action: DeviceLifecycleAction,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), ContestStoreError> {
    let device_id = device_id.as_text();
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                apply_device_lifecycle_in_transaction(
                    connection,
                    &device_id,
                    action,
                    correlation_id,
                    audit_event_id,
                )
            })
        })
        .await
        .map_err(|_| ContestStoreError::AcquireFailed)?
}

fn apply_device_lifecycle_in_transaction(
    connection: &mut SqliteConnection,
    device_id: &str,
    action: DeviceLifecycleAction,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), ContestStoreError> {
    let facts = read_lifecycle_facts(connection, device_id)?;
    let outcome = decide_device_lifecycle(action, facts);
    let audit_result = if outcome.applies {
        DeviceLifecycleAuditResult::Succeeded
    } else {
        DeviceLifecycleAuditResult::Noop
    };
    let event = AuditEvent::device_lifecycle(
        audit_event_id,
        correlation_id,
        device_id.to_owned(),
        action,
        audit_result,
        AuditDetail::DeviceLifecycle {
            resulting_state: outcome.resulting_state.as_persisted(),
            removed_token_count: outcome.removed_token_count,
            revoked_certificate_count: outcome.revoked_certificate_count,
        },
    );
    audit::insert_diesel(connection, &event).map_err(|_| ContestStoreError::AuditInsertFailed)?;

    if outcome.applies {
        apply_lifecycle_mutations(connection, device_id, action, facts.state, outcome)?;
    }
    Ok(())
}

#[derive(QueryableByName)]
struct PersistedLifecycleFactsRow {
    #[diesel(sql_type = Text)]
    persisted_state: String,
    #[diesel(sql_type = BigInt)]
    token_count: i64,
    #[diesel(sql_type = BigInt)]
    non_revoked_certificate_count: i64,
}

fn read_lifecycle_facts(
    connection: &mut SqliteConnection,
    device_id: &str,
) -> Result<DeviceLifecycleFacts, ContestStoreError> {
    let row = diesel::sql_query(
        "SELECT devices.state AS persisted_state, \
         CAST(EXISTS(SELECT 1 FROM device_tokens \
              WHERE device_pk = devices.device_pk) AS INTEGER) AS token_count, \
         (SELECT COUNT(*) FROM gateway_certificates \
          WHERE device_pk = devices.device_pk AND status <> 'revoked') \
             AS non_revoked_certificate_count \
         FROM devices WHERE devices.device_pk = ?",
    )
    .bind::<Text, _>(device_id)
    .get_result::<PersistedLifecycleFactsRow>(connection)
    .optional()
    .map_err(|_| ContestStoreError::ReadFailed)?;
    let Some(row) = row else {
        return Err(ContestStoreError::DeviceNotFound);
    };
    if !matches!(row.token_count, 0 | 1) || row.non_revoked_certificate_count < 0 {
        return Err(ContestStoreError::InvalidPersistedFacts);
    }
    let state = DeviceState::from_persisted(&row.persisted_state)
        .map_err(|_| ContestStoreError::InvalidPersistedFacts)?;
    Ok(DeviceLifecycleFacts {
        state,
        token_count: row.token_count,
        non_revoked_certificate_count: row.non_revoked_certificate_count,
    })
}

fn apply_lifecycle_mutations(
    connection: &mut SqliteConnection,
    device_id: &str,
    action: DeviceLifecycleAction,
    previous_state: DeviceState,
    outcome: DeviceLifecycleOutcome,
) -> Result<(), ContestStoreError> {
    let resulting_state = outcome.resulting_state;
    let state_result = diesel::update(
        devices::table
            .filter(devices::device_pk.eq(device_id))
            .filter(devices::state.ne(resulting_state.as_persisted())),
    )
    .set(devices::state.eq(resulting_state.as_persisted()))
    .execute(connection)
    .map_err(|_| ContestStoreError::MutationFailed)?;
    if state_result != usize::from(previous_state != resulting_state) {
        return Err(ContestStoreError::MutationConflict);
    }
    if matches!(action, DeviceLifecycleAction::Disable) {
        return Ok(());
    }

    let token_result =
        diesel::delete(device_tokens::table.filter(device_tokens::device_pk.eq(device_id)))
            .execute(connection)
            .map_err(|_| ContestStoreError::MutationFailed)?;
    let certificate_result = diesel::update(
        gateway_certificates::table
            .filter(gateway_certificates::device_pk.eq(device_id))
            .filter(gateway_certificates::status.ne("revoked")),
    )
    .set(gateway_certificates::status.eq("revoked"))
    .execute(connection)
    .map_err(|_| ContestStoreError::MutationFailed)?;
    if token_result
        != usize::try_from(outcome.removed_token_count)
            .map_err(|_| ContestStoreError::InvalidPersistedFacts)?
        || certificate_result
            != usize::try_from(outcome.revoked_certificate_count)
                .map_err(|_| ContestStoreError::InvalidPersistedFacts)?
    {
        return Err(ContestStoreError::MutationConflict);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
enum ContestStoreError {
    #[snafu(display("the contest database connection could not be acquired"))]
    AcquireFailed,
    #[snafu(display("the contest transaction failed"))]
    TransactionFailed,
    #[snafu(display("contest current facts could not be read"))]
    ReadFailed,
    #[snafu(display("the Device does not exist"))]
    DeviceNotFound,
    #[snafu(display("persisted Device facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("the Device lifecycle audit could not be inserted"))]
    AuditInsertFailed,
    #[snafu(display("the Device lifecycle mutation failed"))]
    MutationFailed,
    #[snafu(display("the Device lifecycle mutation changed concurrently"))]
    MutationConflict,
}

impl From<diesel::result::Error> for ContestStoreError {
    /// Transaction control is the only stage that reports a raw Diesel error,
    /// and the source is discarded so no SQL text can reach a log or response.
    fn from(_source: diesel::result::Error) -> Self {
        Self::TransactionFailed
    }
}

impl From<ContestStoreError> for ContestError {
    /// The store vocabulary never leaves this module, so every entry point
    /// collapses it here. Both lifecycle callers need the same classification, so
    /// the mapping is total rather than per-caller.
    fn from(source: ContestStoreError) -> Self {
        match source {
            ContestStoreError::DeviceNotFound => Self::DeviceNotFound,
            ContestStoreError::InvalidPersistedFacts => Self::InvalidPersistedFacts,
            ContestStoreError::AcquireFailed
            | ContestStoreError::TransactionFailed
            | ContestStoreError::ReadFailed
            | ContestStoreError::AuditInsertFailed
            | ContestStoreError::MutationFailed
            | ContestStoreError::MutationConflict => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
