use diesel::{
    ExpressionMethods, QueryDsl, RunQueryDsl,
    dsl::sql,
    sql_types::{BigInt, Integer},
    sqlite::SqliteConnection,
};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    application::provisioning::{
        ProvisioningError, ProvisioningWindow, ProvisioningWindowAction, ProvisioningWindowState,
        RecoveryOutcome, RevisionOverflow, recovered_provisioning_window,
    },
    audit::{
        self, AuditEvent, AuditEventId, CorrelationId, EnrollmentExpiryActor,
        ProvisioningWindowAuditResult,
    },
    db::{Database, schema::provisioning_window},
};

/// Runs the startup close-once recovery in a guarded write transaction.
///
/// # Errors
///
/// Returns a redacted [`ProvisioningError`] if the revision overflows, current
/// facts cannot be read, audit insertion fails, the compare-and-swap loses, or
/// commit fails.
pub(crate) async fn recover_provisioning_window(
    database: &Database,
) -> Result<RecoveryOutcome, ProvisioningError> {
    database
        .interact(|connection| {
            connection.immediate_transaction(recover_provisioning_window_in_transaction)
        })
        .await
        .map_err(|_| ProvisioningStoreError::AcquireFailed)?
        .map_err(ProvisioningError::from)
}

/// Reads the singleton provisioning-window current fact without writing.
///
/// # Errors
///
/// Returns a redacted [`ProvisioningError`] when the connection cannot be
/// acquired or the singleton cannot be read and validated.
pub(crate) async fn read_window(
    database: &Database,
) -> Result<ProvisioningWindow, ProvisioningError> {
    database
        .interact(read_provisioning_window)
        .await
        .map_err(|_| ProvisioningStoreError::AcquireFailed)?
        .map_err(ProvisioningError::from)
}

/// Applies an operator-requested open action and its audit in one guarded
/// transaction. An already-open target is an audited repeat-safe no-op.
///
/// # Errors
///
/// Returns a redacted [`ProvisioningError`] when the revision cannot advance or
/// any transaction stage fails.
pub(crate) async fn open_window(
    database: &Database,
    correlation_id: CorrelationId,
) -> Result<ProvisioningWindow, ProvisioningError> {
    open_window_with_ids(
        database,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(ProvisioningError::from)
}

async fn open_window_with_ids(
    database: &Database,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<ProvisioningWindow, ProvisioningStoreError> {
    mutate_window_with_ids(
        database,
        ProvisioningWindowAction::Open,
        correlation_id,
        audit_event_id,
        None,
    )
    .await
}

/// Applies an operator-requested close action and its audit in one guarded
/// transaction. An already-closed target is an audited repeat-safe no-op.
///
/// # Errors
///
/// Returns a redacted [`ProvisioningError`] when the revision cannot advance or
/// any transaction stage fails.
pub(crate) async fn close_window(
    database: &Database,
    correlation_id: CorrelationId,
) -> Result<ProvisioningWindow, ProvisioningError> {
    close_window_with_ids(
        database,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(ProvisioningError::from)
}

async fn close_window_with_ids(
    database: &Database,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
    expiry_audit_event_id: AuditEventId,
) -> Result<ProvisioningWindow, ProvisioningStoreError> {
    mutate_window_with_ids(
        database,
        ProvisioningWindowAction::Close,
        correlation_id,
        audit_event_id,
        Some(expiry_audit_event_id),
    )
    .await
}

async fn mutate_window_with_ids(
    database: &Database,
    action: ProvisioningWindowAction,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
    expiry_audit_event_id: Option<AuditEventId>,
) -> Result<ProvisioningWindow, ProvisioningStoreError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                mutate_window_in_transaction(
                    connection,
                    action,
                    correlation_id,
                    audit_event_id,
                    expiry_audit_event_id,
                )
            })
        })
        .await
        .map_err(|_| ProvisioningStoreError::AcquireFailed)?
}

fn mutate_window_in_transaction(
    connection: &mut SqliteConnection,
    action: ProvisioningWindowAction,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
    expiry_audit_event_id: Option<AuditEventId>,
) -> Result<ProvisioningWindow, ProvisioningStoreError> {
    let current = read_provisioning_window(connection)?;
    let target_state = match action {
        ProvisioningWindowAction::Open => ProvisioningWindowState::Open,
        ProvisioningWindowAction::Close => ProvisioningWindowState::Closed,
    };
    let (next, audit_result) = if current.state == target_state {
        (current, ProvisioningWindowAuditResult::Noop)
    } else {
        let revision = current
            .revision
            .checked_add(1)
            .ok_or(ProvisioningStoreError::RevisionOverflow)?;
        (
            ProvisioningWindow {
                state: target_state,
                revision,
            },
            ProvisioningWindowAuditResult::Succeeded,
        )
    };
    let event = AuditEvent::operator_provisioning_window(
        audit_event_id,
        correlation_id,
        action,
        audit_result,
        current.revision,
        next.revision,
    );
    audit::insert_diesel(connection, &event).map_err(|_| ProvisioningStoreError::AuditFailed)?;

    if audit_result == ProvisioningWindowAuditResult::Succeeded {
        if action == ProvisioningWindowAction::Close {
            expire_live_enrollment_requests(
                connection,
                correlation_id,
                EnrollmentExpiryActor::Operator,
                expiry_audit_event_id.ok_or(ProvisioningStoreError::InvalidExpiryAuditInput)?,
            )?;
        }
        apply_operator_window_cas(connection, current, next, &event)?;
    }
    Ok(next)
}

fn apply_operator_window_cas(
    connection: &mut SqliteConnection,
    expected: ProvisioningWindow,
    next: ProvisioningWindow,
    event: &AuditEvent,
) -> Result<(), ProvisioningStoreError> {
    let result = diesel::update(
        provisioning_window::table
            .filter(provisioning_window::singleton.eq(Some(1_i32)))
            .filter(
                provisioning_window::state.eq(persisted_provisioning_window_state(expected.state)),
            )
            .filter(
                provisioning_window::revision
                    .eq(sql::<Integer>("").bind::<BigInt, _>(expected.revision)),
            ),
    )
    .set((
        provisioning_window::state.eq(persisted_provisioning_window_state(next.state)),
        provisioning_window::revision.eq(sql::<Integer>("").bind::<BigInt, _>(next.revision)),
        provisioning_window::last_audit_event_id.eq(Some(event.audit_event_id_text())),
    ))
    .execute(connection)
    .map_err(|_| ProvisioningStoreError::MutationFailed)?;

    if result != 1 {
        return Err(ProvisioningStoreError::CompareAndSwapConflict);
    }
    Ok(())
}

fn recover_provisioning_window_in_transaction(
    connection: &mut SqliteConnection,
) -> Result<RecoveryOutcome, ProvisioningStoreError> {
    let current = read_provisioning_window(connection)?;

    let Some(next) = recovered_provisioning_window(current)
        .map_err(|RevisionOverflow| ProvisioningStoreError::RevisionOverflow)?
    else {
        return Ok(RecoveryOutcome::AlreadyClosed {
            revision: current.revision,
        });
    };

    let audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    let correlation_id = CorrelationId::from_uuid(Uuid::now_v7());
    let event = AuditEvent::recovery_close(
        audit_event_id,
        correlation_id,
        current.revision,
        next.revision,
    );
    close_open_window(
        connection,
        current,
        next,
        &event,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )?;

    Ok(RecoveryOutcome::Closed {
        previous_revision: current.revision,
        new_revision: next.revision,
        audit_event_id,
    })
}

/// Reads the singleton provisioning-window current fact.
///
/// # Errors
///
/// Returns a redacted [`ProvisioningStoreError`] when the singleton cannot be
/// read or contains invalid facts.
fn read_provisioning_window(
    connection: &mut SqliteConnection,
) -> Result<ProvisioningWindow, ProvisioningStoreError> {
    // SQLite `INTEGER` facts are 64-bit even though Diesel CLI renders this column as `Integer`.
    let (state, revision): (String, i64) = provisioning_window::table
        .select((provisioning_window::state, sql::<BigInt>("revision")))
        .filter(provisioning_window::singleton.eq(Some(1_i32)))
        .first(connection)
        .map_err(|_| ProvisioningStoreError::ReadFailed)?;

    provisioning_window_from_facts(&state, revision)
}

fn provisioning_window_from_facts(
    state: &str,
    revision: i64,
) -> Result<ProvisioningWindow, ProvisioningStoreError> {
    if revision < 0 {
        return Err(ProvisioningStoreError::InvalidCurrentFacts);
    }

    let state = provisioning_window_state_from_persisted(state)?;
    Ok(ProvisioningWindow { state, revision })
}

fn provisioning_window_state_from_persisted(
    state: &str,
) -> Result<ProvisioningWindowState, ProvisioningStoreError> {
    match state {
        "closed" => Ok(ProvisioningWindowState::Closed),
        "open" => Ok(ProvisioningWindowState::Open),
        _ => Err(ProvisioningStoreError::InvalidCurrentFacts),
    }
}

fn persisted_provisioning_window_state(state: ProvisioningWindowState) -> &'static str {
    match state {
        ProvisioningWindowState::Closed => "closed",
        ProvisioningWindowState::Open => "open",
    }
}

/// Inserts the recovery audit and closes the window with a revision CAS.
///
/// Both effects join the caller-owned transaction. Any returned error aborts
/// that transaction, so an audit-only result can never be committed.
///
/// # Errors
///
/// Returns a redacted [`ProvisioningStoreError`] for audit failure, database
/// failure, or a compare-and-swap conflict.
fn close_open_window(
    connection: &mut SqliteConnection,
    expected: ProvisioningWindow,
    next: ProvisioningWindow,
    event: &AuditEvent,
    expiry_correlation_id: CorrelationId,
    expiry_audit_event_id: AuditEventId,
) -> Result<(), ProvisioningStoreError> {
    audit::insert_diesel(connection, event).map_err(|_| ProvisioningStoreError::AuditFailed)?;

    expire_live_enrollment_requests(
        connection,
        expiry_correlation_id,
        EnrollmentExpiryActor::Recovery,
        expiry_audit_event_id,
    )?;

    let result = diesel::update(
        provisioning_window::table
            .filter(provisioning_window::singleton.eq(Some(1_i32)))
            .filter(
                provisioning_window::state.eq(persisted_provisioning_window_state(expected.state)),
            )
            .filter(
                provisioning_window::revision
                    .eq(sql::<Integer>("").bind::<BigInt, _>(expected.revision)),
            ),
    )
    .set((
        provisioning_window::state.eq(persisted_provisioning_window_state(
            ProvisioningWindowState::Closed,
        )),
        provisioning_window::revision.eq(sql::<Integer>("").bind::<BigInt, _>(next.revision)),
        provisioning_window::last_audit_event_id.eq(Some(event.audit_event_id_text())),
    ))
    .execute(connection)
    .map_err(|_| ProvisioningStoreError::MutationFailed)?;

    if result != 1 {
        return Err(ProvisioningStoreError::CompareAndSwapConflict);
    }

    Ok(())
}

fn expire_live_enrollment_requests(
    connection: &mut SqliteConnection,
    correlation_id: CorrelationId,
    actor: EnrollmentExpiryActor,
    audit_event_id: AuditEventId,
) -> Result<i64, ProvisioningStoreError> {
    let expired_count = diesel::sql_query(
        "UPDATE enrollment_requests SET state = 'expired' \
         WHERE state IN ('pending', 'approved', 'rejected')",
    )
    .execute(connection)
    .map_err(|_| ProvisioningStoreError::EnrollmentExpiryFailed)?;
    let expired_count =
        i64::try_from(expired_count).map_err(|_| ProvisioningStoreError::EnrollmentExpiryFailed)?;
    let event = AuditEvent::enrollment_requests_expired(
        audit_event_id,
        correlation_id,
        actor,
        expired_count,
    );
    audit::insert_diesel(connection, &event)
        .map_err(|_| ProvisioningStoreError::ExpiryAuditFailed)?;
    Ok(expired_count)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
enum ProvisioningStoreError {
    #[snafu(display("the database connection could not be acquired"))]
    AcquireFailed,
    #[snafu(display("the provisioning transaction failed"))]
    TransactionFailed,
    #[snafu(display("the provisioning window could not be read"))]
    ReadFailed,
    #[snafu(display("the provisioning window contains invalid current facts"))]
    InvalidCurrentFacts,
    #[snafu(display("the provisioning window revision cannot be incremented"))]
    RevisionOverflow,
    #[snafu(display("the audit event could not be written"))]
    AuditFailed,
    #[snafu(display("the provisioning window could not be mutated"))]
    MutationFailed,
    #[snafu(display("the provisioning window changed concurrently"))]
    CompareAndSwapConflict,
    #[snafu(display("the expiry audit input is invalid"))]
    InvalidExpiryAuditInput,
    #[snafu(display("live Enrollment requests could not be expired"))]
    EnrollmentExpiryFailed,
    #[snafu(display("the Enrollment expiry audit could not be written"))]
    ExpiryAuditFailed,
}

impl From<diesel::result::Error> for ProvisioningStoreError {
    /// Transaction control is the only stage that reports a raw Diesel error,
    /// and the source is discarded so no SQL text can reach a log or response.
    fn from(_source: diesel::result::Error) -> Self {
        Self::TransactionFailed
    }
}

impl From<ProvisioningStoreError> for ProvisioningError {
    /// The store vocabulary never leaves this module. Revision overflow keeps
    /// its dedicated application meaning; every other stage failure is one
    /// internal persistence failure to the startup caller.
    fn from(source: ProvisioningStoreError) -> Self {
        match source {
            ProvisioningStoreError::RevisionOverflow => Self::RevisionOverflow,
            ProvisioningStoreError::AcquireFailed
            | ProvisioningStoreError::TransactionFailed
            | ProvisioningStoreError::ReadFailed
            | ProvisioningStoreError::InvalidCurrentFacts
            | ProvisioningStoreError::AuditFailed
            | ProvisioningStoreError::MutationFailed
            | ProvisioningStoreError::CompareAndSwapConflict
            | ProvisioningStoreError::InvalidExpiryAuditInput
            | ProvisioningStoreError::EnrollmentExpiryFailed
            | ProvisioningStoreError::ExpiryAuditFailed => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
mod tests;
