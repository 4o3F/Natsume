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
pub(crate) mod tests {
    use std::{fs, path::PathBuf};

    use serde_json::Value;
    use snafu::Snafu;
    use uuid::Uuid;

    use diesel::{Connection, connection::SimpleConnection, sql_types::Binary};

    use crate::{db::DatabaseConfig, vault::VaultRecordType};

    use super::*;

    pub(crate) struct TestObserver {
        connection: SqliteConnection,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(crate) struct TestPersistenceSnapshot {
        sessions: i64,
        audits: i64,
        data_version: i64,
        expiries: Vec<String>,
    }

    pub(crate) fn test_observer(
        database_path: &std::path::Path,
    ) -> Result<TestObserver, ContestError> {
        let path = database_path
            .to_str()
            .ok_or(ContestError::PersistenceFailed)?;
        let connection =
            SqliteConnection::establish(path).map_err(|_| ContestError::PersistenceFailed)?;
        Ok(TestObserver { connection })
    }

    pub(crate) async fn test_snapshot(
        database: &Database,
        observer: &mut TestObserver,
    ) -> Result<TestPersistenceSnapshot, ContestError> {
        let (sessions, audits, expiries) = database
            .interact(|connection| {
                let counts = diesel::sql_query(
                    "SELECT (SELECT COUNT(*) FROM operator_sessions) AS sessions, \
                 (SELECT COUNT(*) FROM audit_events) AS audits",
                )
                .get_result::<TestPersistenceCountsRow>(connection)
                .map_err(|_| ContestError::PersistenceFailed)?;
                let expiries = diesel::sql_query(
                    "SELECT expires_at AS value FROM operator_sessions \
                 ORDER BY session_credential_hash",
                )
                .load::<TestTextRow>(connection)
                .map_err(|_| ContestError::PersistenceFailed)?
                .into_iter()
                .map(|row| row.value)
                .collect();
                Ok::<_, ContestError>((counts.sessions, counts.audits, expiries))
            })
            .await
            .map_err(|_| ContestError::PersistenceFailed)??;
        let data_version = diesel::dsl::sql::<BigInt>("PRAGMA data_version")
            .get_result(&mut observer.connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
        Ok(TestPersistenceSnapshot {
            sessions,
            audits,
            data_version,
            expiries,
        })
    }

    pub(crate) async fn test_expire_all_sessions(database: &Database) -> Result<(), ContestError> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "UPDATE operator_sessions \
                 SET expires_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 second')",
                )
                .execute(connection)
                .map(|_| ())
                .map_err(|_| ContestError::PersistenceFailed)
            })
            .await
            .map_err(|_| ContestError::PersistenceFailed)?
    }

    pub(crate) async fn test_seed_current_facts(
        database: &Database,
        vault_pointer_canary: &str,
        hardware_id_canary: &str,
    ) -> Result<(), ContestError> {
        let vault_pointer_canary = vault_pointer_canary.to_owned();
        let hardware_id_canary = hardware_id_canary.to_owned();
        let record_type = VaultRecordType::AccountCredential.as_str();
        database
        .interact(move |connection| {
            diesel::sql_query(
                "INSERT INTO server_vault_records \
                 (vault_record_id, record_type, subject_id, nonce, ciphertext) VALUES \
                 (?, ?, 'account-a', x'01', x'02'), \
                 ('vault-record-b', ?, 'account-b', x'03', x'04')",
            )
            .bind::<Text, _>(&vault_pointer_canary)
            .bind::<Text, _>(record_type)
            .bind::<Text, _>(record_type)
            .execute(connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
            diesel::sql_query(
                "INSERT INTO seats (seat_id, seat_code) VALUES \
                 ('seat-b', 'B-02'), ('seat-a', 'A-01')",
            )
            .execute(connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
            diesel::sql_query(
                "INSERT INTO accounts \
                 (account_id, domjudge_username, credential_vault_record_id, credential_revision) VALUES \
                 ('account-b', 'team-beta', 'vault-record-b', 7), \
                 ('account-a', 'team-alpha', ?, 3)",
            )
            .bind::<Text, _>(&vault_pointer_canary)
            .execute(connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
            diesel::sql_query(
                "INSERT INTO devices \
                 (device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES \
                 ('device-b', 'machine-hardware-b', 'medium', 'disabled'), \
                 ('device-a', ?, 'strong', 'enrolled')",
            )
            .bind::<Text, _>(&hardware_id_canary)
            .execute(connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
            diesel::sql_query(
                "INSERT INTO device_bindings (seat_id, device_pk, binding_revision) VALUES \
                 ('seat-b', 'device-b', 11), ('seat-a', 'device-a', 11)",
            )
            .execute(connection)
            .map(|_| ())
            .map_err(|_| ContestError::PersistenceFailed)
        })
        .await
        .map_err(|_| ContestError::PersistenceFailed)?
    }

    pub(crate) async fn test_seed_lifecycle_device(
        database: &Database,
        device_id: &str,
        state: &str,
        with_token: bool,
        certificate_status: &str,
    ) -> Result<(), ContestError> {
        let device_id = device_id.to_owned();
        let state = state.to_owned();
        let certificate_status = certificate_status.to_owned();
        database
        .interact(move |connection| {
            let enrollment_id = format!("enrollment-{device_id}");
            let seat_id = format!("seat-{device_id}");
            let hardware_id = format!("hardware-secret-{device_id}");
            diesel::sql_query("INSERT INTO seats (seat_id, seat_code) VALUES (?, ?)")
                .bind::<Text, _>(&seat_id)
                .bind::<Text, _>(format!("code-{device_id}"))
                .execute(connection)
                .map_err(|_| ContestError::PersistenceFailed)?;
            diesel::sql_query(
                "INSERT INTO devices \
                 (device_pk, machine_hardware_id, hardware_identity_quality, state) \
                 VALUES (?, ?, 'strong', ?)",
            )
            .bind::<Text, _>(&device_id)
            .bind::<Text, _>(&hardware_id)
            .bind::<Text, _>(&state)
            .execute(connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
            diesel::sql_query(
                "INSERT INTO enrollment_requests \
                 (enrollment_request_id, machine_hardware_id, hardware_identity_quality, \
                  gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, source_ip, \
                  state, created_at) \
                 VALUES (?, ?, 'strong', ?, ?, 'test-client', 1, '192.0.2.1', 'pending', \
                         '2026-08-08T00:00:00.000Z')",
            )
            .bind::<Text, _>(&enrollment_id)
            .bind::<Text, _>(&hardware_id)
            .bind::<Binary, _>(b"certificate-material-secret-canary".as_slice())
            .bind::<Binary, _>(b"spki-hash-secret-canary-12345678".as_slice())
            .execute(connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
            if with_token {
                let mut token_hash = *b"token-hash-secret-canary-1234567";
                let unique_byte = device_id
                    .as_bytes()
                    .last()
                    .copied()
                    .ok_or(ContestError::InvalidPersistedFacts)?;
                token_hash[31] = unique_byte;
                diesel::sql_query(
                    "INSERT INTO device_tokens (device_pk, enrollment_request_id, token_hash) \
                     VALUES (?, ?, ?)",
                )
                .bind::<Text, _>(&device_id)
                .bind::<Text, _>(&enrollment_id)
                .bind::<Binary, _>(token_hash.as_slice())
                .execute(connection)
                .map_err(|_| ContestError::PersistenceFailed)?;
            }
            diesel::sql_query(
                "INSERT INTO gateway_certificates \
                 (certificate_id, device_pk, enrollment_request_id, serial, spki_sha256, not_after, status) \
                 VALUES (?, ?, ?, ?, ?, '2027-08-08T00:00:00.000Z', ?)",
            )
            .bind::<Text, _>(format!("certificate-{device_id}"))
            .bind::<Text, _>(&device_id)
            .bind::<Text, _>(&enrollment_id)
            .bind::<Text, _>(format!("certificate-serial-secret-{device_id}"))
            .bind::<Binary, _>(b"spki-secret-canary-1234567890123".as_slice())
            .bind::<Text, _>(&certificate_status)
            .execute(connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
            diesel::sql_query(
                "INSERT INTO device_bindings (seat_id, device_pk, binding_revision) \
                 VALUES (?, ?, 43)",
            )
            .bind::<Text, _>(&seat_id)
            .bind::<Text, _>(&device_id)
            .execute(connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
            diesel::sql_query(
                "UPDATE revision_counters SET configuration_revision = 41, \
                 binding_revision = 43 WHERE singleton = 1",
            )
            .execute(connection)
            .map(|_| ())
            .map_err(|_| ContestError::PersistenceFailed)
        })
        .await
        .map_err(|_| ContestError::PersistenceFailed)?
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(crate) struct TestLifecycleSnapshot {
        pub(crate) state: String,
        pub(crate) token_count: i64,
        pub(crate) certificate_statuses: Vec<String>,
        pub(crate) binding_revision: i64,
        pub(crate) configuration_revision: i64,
        pub(crate) global_binding_revision: i64,
        pub(crate) command_count: i64,
    }

    pub(crate) async fn test_lifecycle_snapshot(
        database: &Database,
        device_id: &str,
    ) -> Result<TestLifecycleSnapshot, ContestError> {
        let device_id = device_id.to_owned();
        database
            .interact(move |connection| {
                let row = diesel::sql_query(
                    "SELECT devices.state AS state, \
                 (SELECT COUNT(*) FROM device_tokens WHERE device_pk = devices.device_pk) \
                     AS token_count, \
                 device_bindings.binding_revision AS binding_revision, \
                 revision_counters.configuration_revision AS configuration_revision, \
                 revision_counters.binding_revision AS global_binding_revision \
                 FROM devices \
                 JOIN device_bindings ON device_bindings.device_pk = devices.device_pk \
                 JOIN revision_counters ON revision_counters.singleton = 1 \
                 WHERE devices.device_pk = ?",
                )
                .bind::<Text, _>(&device_id)
                .get_result::<TestLifecycleSnapshotRow>(connection)
                .map_err(|_| ContestError::PersistenceFailed)?;
                let certificate_statuses = diesel::sql_query(
                    "SELECT status AS value FROM gateway_certificates WHERE device_pk = ? \
                 ORDER BY certificate_id",
                )
                .bind::<Text, _>(&device_id)
                .load::<TestTextRow>(connection)
                .map_err(|_| ContestError::PersistenceFailed)?
                .into_iter()
                .map(|status| status.value)
                .collect();
                let command_count =
                    diesel::sql_query("SELECT COUNT(*) AS value FROM commands WHERE device_pk = ?")
                        .bind::<Text, _>(&device_id)
                        .get_result::<TestIntegerRow>(connection)
                        .map_err(|_| ContestError::PersistenceFailed)?
                        .value;
                Ok(TestLifecycleSnapshot {
                    state: row.state,
                    token_count: row.token_count,
                    certificate_statuses,
                    binding_revision: row.binding_revision,
                    configuration_revision: row.configuration_revision,
                    global_binding_revision: row.global_binding_revision,
                    command_count,
                })
            })
            .await
            .map_err(|_| ContestError::PersistenceFailed)?
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(crate) struct TestLifecycleAudit {
        pub(crate) actor: String,
        pub(crate) action: String,
        pub(crate) resource_type: String,
        pub(crate) resource_id: String,
        pub(crate) result: String,
        pub(crate) reason: String,
        pub(crate) detail: String,
        pub(crate) complete_row: String,
    }

    pub(crate) async fn test_latest_lifecycle_audit(
        database: &Database,
        device_id: &str,
    ) -> Result<TestLifecycleAudit, ContestError> {
        let device_id = device_id.to_owned();
        database
            .interact(move |connection| {
                let row = diesel::sql_query(
                    "SELECT actor, action_kind AS action, resource_type, resource_id, result, \
                 reason_code AS reason, redacted_detail_json AS detail, \
                 audit_event_id || occurred_at || actor || action_kind || resource_type || \
                 resource_id || result || COALESCE(reason_code, '') || correlation_id || \
                 COALESCE(group_correlation_id, '') || redacted_detail_json AS complete_row \
                 FROM audit_events WHERE resource_type = 'device' AND resource_id = ? \
                 ORDER BY rowid DESC LIMIT 1",
                )
                .bind::<Text, _>(&device_id)
                .get_result::<TestLifecycleAuditRow>(connection)
                .map_err(|_| ContestError::PersistenceFailed)?;
                Ok(TestLifecycleAudit {
                    actor: row.actor,
                    action: row.action,
                    resource_type: row.resource_type,
                    resource_id: row.resource_id,
                    result: row.result,
                    reason: row.reason,
                    detail: row.detail,
                    complete_row: row.complete_row,
                })
            })
            .await
            .map_err(|_| ContestError::PersistenceFailed)?
    }

    pub(crate) async fn test_lifecycle_audit_count(
        database: &Database,
        device_id: &str,
    ) -> Result<i64, ContestError> {
        let device_id = device_id.to_owned();
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "SELECT COUNT(*) AS value FROM audit_events \
                 WHERE resource_type = 'device' AND resource_id = ?",
                )
                .bind::<Text, _>(&device_id)
                .get_result::<TestIntegerRow>(connection)
                .map(|row| row.value)
                .map_err(|_| ContestError::PersistenceFailed)
            })
            .await
            .map_err(|_| ContestError::PersistenceFailed)?
    }

    pub(crate) fn test_data_version(observer: &mut TestObserver) -> Result<i64, ContestError> {
        diesel::dsl::sql::<BigInt>("PRAGMA data_version")
            .get_result(&mut observer.connection)
            .map_err(|_| ContestError::PersistenceFailed)
    }

    pub(crate) async fn test_reserve_audit_id(
        database: &Database,
        audit_event_id: AuditEventId,
    ) -> Result<(), ContestError> {
        database
            .interact(move |connection| {
                let event = AuditEvent::device_lifecycle(
                    audit_event_id,
                    CorrelationId::from_uuid(Uuid::now_v7()),
                    "reserved-device".to_owned(),
                    DeviceLifecycleAction::Disable,
                    DeviceLifecycleAuditResult::Noop,
                    AuditDetail::DeviceLifecycle {
                        resulting_state: "disabled",
                        removed_token_count: 0,
                        revoked_certificate_count: 0,
                    },
                );
                audit::insert_diesel(connection, &event)
                    .map_err(|_| ContestError::PersistenceFailed)
            })
            .await
            .map_err(|_| ContestError::PersistenceFailed)?
    }

    #[derive(QueryableByName)]
    struct TestPersistenceCountsRow {
        #[diesel(sql_type = BigInt)]
        sessions: i64,
        #[diesel(sql_type = BigInt)]
        audits: i64,
    }

    #[derive(QueryableByName)]
    struct TestTextRow {
        #[diesel(sql_type = Text)]
        value: String,
    }

    #[derive(QueryableByName)]
    struct TestIntegerRow {
        #[diesel(sql_type = BigInt)]
        value: i64,
    }

    #[derive(QueryableByName)]
    struct TestLifecycleSnapshotRow {
        #[diesel(sql_type = Text)]
        state: String,
        #[diesel(sql_type = BigInt)]
        token_count: i64,
        #[diesel(sql_type = BigInt)]
        binding_revision: i64,
        #[diesel(sql_type = BigInt)]
        configuration_revision: i64,
        #[diesel(sql_type = BigInt)]
        global_binding_revision: i64,
    }

    #[derive(QueryableByName)]
    struct TestLifecycleAuditRow {
        #[diesel(sql_type = Text)]
        actor: String,
        #[diesel(sql_type = Text)]
        action: String,
        #[diesel(sql_type = Text)]
        resource_type: String,
        #[diesel(sql_type = Text)]
        resource_id: String,
        #[diesel(sql_type = Text)]
        result: String,
        #[diesel(sql_type = Text)]
        reason: String,
        #[diesel(sql_type = Text)]
        detail: String,
        #[diesel(sql_type = Text)]
        complete_row: String,
    }
    const ENROLLED_DEVICE: &str = "01900000-0000-7000-8000-000000000101";
    const DISABLED_DEVICE: &str = "01900000-0000-7000-8000-000000000102";
    const REVOKED_DEVICE: &str = "01900000-0000-7000-8000-000000000103";
    const PARTIAL_DEVICE: &str = "01900000-0000-7000-8000-000000000104";
    const CERTIFICATE_PARTIAL_DEVICE: &str = "01900000-0000-7000-8000-000000000105";

    #[tokio::test]
    async fn revoke_converges_then_records_a_business_zero_write_noop() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        test_seed_lifecycle_device(
            &fixture.database,
            ENROLLED_DEVICE,
            "enrolled",
            true,
            "active",
        )
        .await
        .map_err(|_| TestFailure::FixtureFailed)?;
        let device_id = device_id(ENROLLED_DEVICE)?;

        apply(&fixture.database, &device_id, DeviceLifecycleAction::Revoke).await?;
        let applied = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let succeeded = test_latest_lifecycle_audit(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        verify_revoke_effect(&applied)?;
        verify_audit(
            &succeeded,
            "revoke_device",
            "succeeded",
            "operator_requested",
            r#"{"resulting_state":"revoked","removed_token_count":1,"revoked_certificate_count":1}"#,
        )?;
        verify_audit_is_allowlisted(&succeeded, ENROLLED_DEVICE)?;

        let mut observer = test_observer(&fixture.path).map_err(|_| TestFailure::EvidenceFailed)?;
        let version_before =
            test_data_version(&mut observer).map_err(|_| TestFailure::EvidenceFailed)?;
        apply(&fixture.database, &device_id, DeviceLifecycleAction::Revoke).await?;
        let after_noop = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let version_after =
            test_data_version(&mut observer).map_err(|_| TestFailure::EvidenceFailed)?;
        let noop = test_latest_lifecycle_audit(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let audit_count = test_lifecycle_audit_count(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        if applied != after_noop || audit_count != 2 || version_after == version_before {
            return Err(TestFailure::NoopChangedBusinessFacts);
        }
        verify_audit(
            &noop,
            "revoke_device",
            "noop",
            "target_already_satisfied",
            r#"{"resulting_state":"revoked","removed_token_count":0,"revoked_certificate_count":0}"#,
        )
    }

    #[tokio::test]
    async fn disable_is_repeat_safe_and_never_weakens_revoked() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        for (id, state, token, certificate) in [
            (ENROLLED_DEVICE, "enrolled", true, "active"),
            (REVOKED_DEVICE, "revoked", false, "revoked"),
        ] {
            test_seed_lifecycle_device(&fixture.database, id, state, token, certificate)
                .await
                .map_err(|_| TestFailure::FixtureFailed)?;
        }

        let enrolled = device_id(ENROLLED_DEVICE)?;
        apply(&fixture.database, &enrolled, DeviceLifecycleAction::Disable).await?;
        let disabled_once = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let disabled_succeeded = test_latest_lifecycle_audit(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        apply(&fixture.database, &enrolled, DeviceLifecycleAction::Disable).await?;
        let disabled_twice = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let disabled_noop = test_latest_lifecycle_audit(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let disable_audit_count = test_lifecycle_audit_count(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        if disabled_once.state != "disabled"
            || disabled_once.token_count != 1
            || disabled_once.certificate_statuses != ["active"]
            || disabled_once.binding_revision != 43
            || disabled_once.configuration_revision != 41
            || disabled_once.global_binding_revision != 43
            || disabled_once.command_count != 0
            || disabled_once != disabled_twice
            || disable_audit_count != 2
        {
            return Err(TestFailure::DisableTransitionChanged);
        }
        verify_audit(
            &disabled_succeeded,
            "disable_device",
            "succeeded",
            "operator_requested",
            r#"{"resulting_state":"disabled","removed_token_count":0,"revoked_certificate_count":0}"#,
        )?;
        verify_audit(
            &disabled_noop,
            "disable_device",
            "noop",
            "target_already_satisfied",
            r#"{"resulting_state":"disabled","removed_token_count":0,"revoked_certificate_count":0}"#,
        )?;

        let revoked = device_id(REVOKED_DEVICE)?;
        let revoked_before = test_lifecycle_snapshot(&fixture.database, REVOKED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        apply(&fixture.database, &revoked, DeviceLifecycleAction::Disable).await?;
        let revoked_after = test_lifecycle_snapshot(&fixture.database, REVOKED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let revoked_noop = test_latest_lifecycle_audit(&fixture.database, REVOKED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        if revoked_before != revoked_after
            || revoked_after.state != "revoked"
            || revoked_after.token_count != 0
            || revoked_after.certificate_statuses != ["revoked"]
            || revoked_after.binding_revision != 43
            || revoked_after.configuration_revision != 41
            || revoked_after.global_binding_revision != 43
            || revoked_after.command_count != 0
        {
            return Err(TestFailure::StrongerStateWasWeakened);
        }
        verify_audit(
            &revoked_noop,
            "disable_device",
            "noop",
            "target_already_satisfied",
            r#"{"resulting_state":"revoked","removed_token_count":0,"revoked_certificate_count":0}"#,
        )
    }

    #[tokio::test]
    async fn revoke_converges_from_disabled_and_each_partial_target() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        for (id, state, token, certificate) in [
            (DISABLED_DEVICE, "disabled", true, "active"),
            (PARTIAL_DEVICE, "revoked", true, "revoked"),
            (CERTIFICATE_PARTIAL_DEVICE, "revoked", false, "active"),
        ] {
            test_seed_lifecycle_device(&fixture.database, id, state, token, certificate)
                .await
                .map_err(|_| TestFailure::FixtureFailed)?;
        }
        for (id, detail) in [
            (
                DISABLED_DEVICE,
                r#"{"resulting_state":"revoked","removed_token_count":1,"revoked_certificate_count":1}"#,
            ),
            (
                PARTIAL_DEVICE,
                r#"{"resulting_state":"revoked","removed_token_count":1,"revoked_certificate_count":0}"#,
            ),
            (
                CERTIFICATE_PARTIAL_DEVICE,
                r#"{"resulting_state":"revoked","removed_token_count":0,"revoked_certificate_count":1}"#,
            ),
        ] {
            let parsed = device_id(id)?;
            apply(&fixture.database, &parsed, DeviceLifecycleAction::Revoke).await?;
            let snapshot = test_lifecycle_snapshot(&fixture.database, id)
                .await
                .map_err(|_| TestFailure::EvidenceFailed)?;
            let audit = test_latest_lifecycle_audit(&fixture.database, id)
                .await
                .map_err(|_| TestFailure::EvidenceFailed)?;
            verify_revoke_effect(&snapshot)?;
            if audit.result != "succeeded" {
                return Err(TestFailure::PartialRevokeWasReportedAsNoop);
            }
            if audit.detail != detail {
                return Err(TestFailure::AuditChanged);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn failed_audit_insert_rolls_back_the_full_revoke_effect() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        test_seed_lifecycle_device(
            &fixture.database,
            ENROLLED_DEVICE,
            "enrolled",
            true,
            "active",
        )
        .await
        .map_err(|_| TestFailure::FixtureFailed)?;
        let duplicate_audit_id = AuditEventId::from_uuid(Uuid::now_v7());
        test_reserve_audit_id(&fixture.database, duplicate_audit_id)
            .await
            .map_err(|_| TestFailure::FixtureFailed)?;
        let before = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let before_audits = test_lifecycle_audit_count(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let result = apply_device_lifecycle_with_audit_id(
            &fixture.database,
            &device_id(ENROLLED_DEVICE)?,
            DeviceLifecycleAction::Revoke,
            correlation_id(),
            duplicate_audit_id,
        )
        .await;
        let after = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let after_audits = test_lifecycle_audit_count(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        if !matches!(result, Err(ContestStoreError::AuditInsertFailed))
            || after != before
            || after_audits != before_audits
        {
            return Err(TestFailure::AuditFailureDidNotRollBack);
        }
        Ok(())
    }

    #[tokio::test]
    async fn persisted_device_vocabulary_outside_the_frozen_set_fails_closed()
    -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        test_seed_lifecycle_device(
            &fixture.database,
            ENROLLED_DEVICE,
            "enrolled",
            true,
            "active",
        )
        .await
        .map_err(|_| TestFailure::FixtureFailed)?;
        if list_devices(&fixture.database).await.is_err() {
            return Err(TestFailure::FrozenVocabularyWasRejected);
        }

        for statement in [
            "UPDATE devices SET state = 'quarantined'",
            "UPDATE devices SET state = 'enrolled', \
             hardware_identity_quality = 'excellent'",
        ] {
            poison_device_vocabulary(&fixture.path, statement)?;
            if !matches!(
                list_devices(&fixture.database).await,
                Err(ContestError::InvalidPersistedFacts)
            ) {
                return Err(TestFailure::UnfrozenVocabularyWasAccepted);
            }
        }
        Ok(())
    }

    /// The migration's CHECK constraints make an out-of-vocabulary value
    /// unreachable through the pool, so the fixture writes one on a separate
    /// connection that has constraint checking disabled.
    fn poison_device_vocabulary(
        database_path: &std::path::Path,
        statement: &str,
    ) -> Result<(), TestFailure> {
        let mut observer = test_observer(database_path).map_err(|_| TestFailure::FixtureFailed)?;
        observer
            .connection
            .batch_execute("PRAGMA ignore_check_constraints = ON;")
            .map_err(|_| TestFailure::FixtureFailed)?;
        diesel::sql_query(statement)
            .execute(&mut observer.connection)
            .map(|_| ())
            .map_err(|_| TestFailure::FixtureFailed)
    }

    fn verify_revoke_effect(snapshot: &TestLifecycleSnapshot) -> Result<(), TestFailure> {
        if snapshot.state != "revoked"
            || snapshot.token_count != 0
            || snapshot.certificate_statuses != ["revoked"]
            || snapshot.binding_revision != 43
            || snapshot.configuration_revision != 41
            || snapshot.global_binding_revision != 43
            || snapshot.command_count != 0
        {
            return Err(TestFailure::RevokeEffectChanged);
        }
        Ok(())
    }

    fn verify_audit(
        audit: &TestLifecycleAudit,
        action: &str,
        result: &str,
        reason: &str,
        detail: &str,
    ) -> Result<(), TestFailure> {
        let parsed: Value =
            serde_json::from_str(&audit.detail).map_err(|_| TestFailure::AuditChanged)?;
        let object = parsed.as_object().ok_or(TestFailure::AuditChanged)?;
        if audit.actor != "operator:self"
            || audit.action != action
            || audit.resource_type != "device"
            || audit.result != result
            || audit.reason != reason
            || audit.detail != detail
            || object.len() != 3
            || !object.contains_key("resulting_state")
            || !object.contains_key("removed_token_count")
            || !object.contains_key("revoked_certificate_count")
        {
            return Err(TestFailure::AuditChanged);
        }
        Ok(())
    }

    fn verify_audit_is_allowlisted(
        audit: &TestLifecycleAudit,
        device_id: &str,
    ) -> Result<(), TestFailure> {
        for forbidden in [
            format!("hardware-secret-{device_id}"),
            format!("certificate-serial-secret-{device_id}"),
            "certificate-material-secret-canary".to_owned(),
            "token-hash-secret-canary-".to_owned(),
            "spki-hash-secret-canary-".to_owned(),
            "spki-secret-canary-".to_owned(),
            "token_hash".to_owned(),
            "machine_hardware_id".to_owned(),
            "serial".to_owned(),
        ] {
            if audit.complete_row.contains(&forbidden) || audit.detail.contains(&forbidden) {
                return Err(TestFailure::AuditLeakedForbiddenEvidence);
            }
        }
        if audit.resource_id != device_id {
            return Err(TestFailure::AuditChanged);
        }
        Ok(())
    }

    fn device_id(value: &str) -> Result<DeviceId, TestFailure> {
        DeviceId::parse(value).map_err(|_| TestFailure::FixtureFailed)
    }

    async fn apply(
        database: &Database,
        device_id: &DeviceId,
        action: DeviceLifecycleAction,
    ) -> Result<(), TestFailure> {
        apply_device_lifecycle(database, device_id, action, correlation_id())
            .await
            .map_err(|_| TestFailure::LifecycleFailed)
    }

    fn correlation_id() -> CorrelationId {
        CorrelationId::from_uuid(Uuid::now_v7())
    }

    struct TestDatabase {
        database: Database,
        path: PathBuf,
    }

    impl TestDatabase {
        async fn new() -> Result<Self, TestFailure> {
            let path = std::env::temp_dir().join(format!(
                "natsume-contest-lifecycle-test-{}.sqlite3",
                Uuid::now_v7()
            ));
            let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
                .await
                .map_err(|_| TestFailure::FixtureFailed)?;
            Ok(Self { database, path })
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            let _database_result = fs::remove_file(&self.path);
            let _wal_result = fs::remove_file(format!("{}-wal", self.path.display()));
            let _shm_result = fs::remove_file(format!("{}-shm", self.path.display()));
        }
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the lifecycle database fixture failed"))]
        FixtureFailed,
        #[snafu(display("the lifecycle operation failed"))]
        LifecycleFailed,
        #[snafu(display("lifecycle persistence evidence could not be read"))]
        EvidenceFailed,
        #[snafu(display("the revoke effect changed"))]
        RevokeEffectChanged,
        #[snafu(display("the repeat revoke changed business facts"))]
        NoopChangedBusinessFacts,
        #[snafu(display("the disable transition changed"))]
        DisableTransitionChanged,
        #[snafu(display("the stronger Device state was weakened"))]
        StrongerStateWasWeakened,
        #[snafu(display("a partial revoke was reported as a no-op"))]
        PartialRevokeWasReportedAsNoop,
        #[snafu(display("the lifecycle audit envelope changed"))]
        AuditChanged,
        #[snafu(display("forbidden lifecycle evidence escaped into audit"))]
        AuditLeakedForbiddenEvidence,
        #[snafu(display("an audit failure did not roll back lifecycle state"))]
        AuditFailureDidNotRollBack,
        #[snafu(display("the frozen persisted Device vocabulary was rejected"))]
        FrozenVocabularyWasRejected,
        #[snafu(display("an unfrozen persisted Device vocabulary value was accepted"))]
        UnfrozenVocabularyWasAccepted,
    }
}
