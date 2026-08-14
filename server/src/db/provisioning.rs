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
        ProvisioningError, ProvisioningWindow, ProvisioningWindowState, RecoveryOutcome,
        RevisionOverflow, recovered_provisioning_window,
    },
    audit::{self, AuditEvent, AuditEventId, CorrelationId},
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
    let event = AuditEvent::recovery_close(
        audit_event_id,
        CorrelationId::from_uuid(Uuid::now_v7()),
        current.revision,
        next.revision,
    );
    close_open_window(connection, current, next, &event)?;

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
) -> Result<(), ProvisioningStoreError> {
    audit::insert_diesel(connection, event).map_err(|_| ProvisioningStoreError::AuditFailed)?;

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
            | ProvisioningStoreError::CompareAndSwapConflict => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use diesel::{
        QueryableByName, RunQueryDsl,
        sql_types::{BigInt, Text},
    };
    use snafu::Snafu;
    use uuid::Uuid;

    use crate::{
        application::provisioning::{ProvisioningWindow, ProvisioningWindowState},
        audit::{AuditEvent, AuditEventId, CorrelationId},
        db::{Database, DatabaseConfig},
    };

    use super::{ProvisioningStoreError, close_open_window};

    /// The caller owns the only transaction, so a typed inner failure must abort
    /// it and leave neither the audit nor the window change persisted.
    #[tokio::test]
    async fn close_open_window_failures_persist_no_partial_effect() -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, true))
            .await
            .map_err(|_| TestFailure::DatabaseCreationFailed)?;
        let opening_audit_id = Uuid::now_v7();
        seed_open_window(&database, opening_audit_id).await?;

        // A duplicate audit event ID fails the audit insert.
        expect_rolled_back_failure(
            &database,
            opening_audit_id,
            1,
            ProvisioningStoreError::AuditFailed,
            TestFailure::DuplicateAuditFailureWasNotTyped,
        )
        .await?;

        // A stale expected revision loses the compare-and-swap.
        let stale_audit_id = Uuid::now_v7();
        expect_rolled_back_failure(
            &database,
            stale_audit_id,
            0,
            ProvisioningStoreError::CompareAndSwapConflict,
            TestFailure::CompareAndSwapConflictWasNotTyped,
        )
        .await?;

        let stale_audit_id_text = stale_audit_id.to_string();
        let (window, stale_audit_count) = database
            .interact(move |connection| {
                let window = diesel::sql_query(
                    "SELECT state, revision, last_audit_event_id \
                     FROM provisioning_window WHERE singleton = 1",
                )
                .get_result::<WindowRow>(connection)
                .map_err(|_| TestFailure::WindowWasNotReadable)?;
                let stale_audit_count = diesel::sql_query(
                    "SELECT COUNT(*) AS value FROM audit_events WHERE audit_event_id = ?",
                )
                .bind::<Text, _>(&stale_audit_id_text)
                .get_result::<CountRow>(connection)
                .map_err(|_| TestFailure::AuditWasNotReadable)?
                .value;
                Ok((window, stale_audit_count))
            })
            .await
            .map_err(|_| TestFailure::DieselInteractionFailed)??;
        if window.state != "open"
            || window.revision != 1
            || window.last_audit_event_id != opening_audit_id.to_string()
        {
            return Err(TestFailure::WindowChangedAfterFailure);
        }
        if stale_audit_count != 0 {
            return Err(TestFailure::StaleAuditWasWritten);
        }
        Ok(())
    }

    /// Drives one failing close inside a caller-owned transaction and rolls it
    /// back, exactly as `recover_provisioning_window` does.
    async fn expect_rolled_back_failure(
        database: &Database,
        audit_event_id: Uuid,
        expected_revision: i64,
        expected_error: ProvisioningStoreError,
        typed_failure: TestFailure,
    ) -> Result<(), TestFailure> {
        let next_revision = expected_revision + 1;
        let event = AuditEvent::recovery_close(
            AuditEventId::from_uuid(audit_event_id),
            CorrelationId::from_uuid(Uuid::now_v7()),
            expected_revision,
            next_revision,
        );
        database
            .interact(move |connection| {
                // The typed inner failure is returned out of the closure, so the
                // transaction rolls back exactly as the recovery path does.
                let result = connection.immediate_transaction(|connection| {
                    close_open_window(
                        connection,
                        ProvisioningWindow {
                            state: ProvisioningWindowState::Open,
                            revision: expected_revision,
                        },
                        ProvisioningWindow {
                            state: ProvisioningWindowState::Closed,
                            revision: next_revision,
                        },
                        &event,
                    )
                });
                if result != Err(expected_error) {
                    return Err(typed_failure);
                }
                Ok(())
            })
            .await
            .map_err(|_| TestFailure::DieselInteractionFailed)?
    }

    async fn seed_open_window(
        database: &Database,
        audit_event_id: Uuid,
    ) -> Result<(), TestFailure> {
        let audit_event_id = audit_event_id.to_string();
        let correlation_id = Uuid::now_v7().to_string();
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "INSERT INTO audit_events (audit_event_id, occurred_at, actor, action_kind, \
                     resource_type, resource_id, result, reason_code, correlation_id, \
                     group_correlation_id, redacted_detail_json) VALUES (?, \
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'system:test', \
                     'open_provisioning_window', 'provisioning_window', NULL, 'succeeded', \
                     NULL, ?, NULL, '{}')",
                )
                .bind::<Text, _>(&audit_event_id)
                .bind::<Text, _>(&correlation_id)
                .execute(connection)
                .map_err(|_| TestFailure::AuditFixtureInsertFailed)?;
                let updated = diesel::sql_query(
                    "UPDATE provisioning_window SET state = 'open', revision = 1, \
                     last_audit_event_id = ? WHERE singleton = 1",
                )
                .bind::<Text, _>(&audit_event_id)
                .execute(connection)
                .map_err(|_| TestFailure::WindowFixtureUpdateFailed)?;
                if updated != 1 {
                    return Err(TestFailure::WindowFixtureUpdateWasNotExact);
                }
                Ok(())
            })
            .await
            .map_err(|_| TestFailure::DieselInteractionFailed)?
    }

    #[derive(QueryableByName)]
    struct WindowRow {
        #[diesel(sql_type = Text)]
        state: String,
        #[diesel(sql_type = BigInt)]
        revision: i64,
        #[diesel(sql_type = Text)]
        last_audit_event_id: String,
    }

    #[derive(QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = BigInt)]
        value: i64,
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the test database could not be created"))]
        DatabaseCreationFailed,
        #[snafu(display("the test Diesel interaction failed"))]
        DieselInteractionFailed,
        #[snafu(display("the test audit fixture could not be inserted"))]
        AuditFixtureInsertFailed,
        #[snafu(display("the test provisioning window could not be opened"))]
        WindowFixtureUpdateFailed,
        #[snafu(display("the test provisioning window update was not exact"))]
        WindowFixtureUpdateWasNotExact,
        #[snafu(display("the duplicate audit failure was not typed"))]
        DuplicateAuditFailureWasNotTyped,
        #[snafu(display("the compare-and-swap conflict was not typed"))]
        CompareAndSwapConflictWasNotTyped,
        #[snafu(display("the provisioning window was not readable"))]
        WindowWasNotReadable,
        #[snafu(display("the provisioning window changed after a rolled-back failure"))]
        WindowChangedAfterFailure,
        #[snafu(display("the audit evidence was not readable"))]
        AuditWasNotReadable,
        #[snafu(display("the stale audit event was written"))]
        StaleAuditWasWritten,
    }

    struct DatabaseFixture {
        path: PathBuf,
    }

    impl DatabaseFixture {
        fn new() -> Self {
            Self {
                path: std::env::temp_dir().join(format!(
                    "natsume-provisioning-close-window-test-{}.sqlite3",
                    Uuid::now_v7()
                )),
            }
        }

        fn wal_path(&self) -> PathBuf {
            PathBuf::from(format!("{}-wal", self.path.display()))
        }

        fn shm_path(&self) -> PathBuf {
            PathBuf::from(format!("{}-shm", self.path.display()))
        }
    }

    impl Drop for DatabaseFixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_file(self.wal_path());
            let _ = fs::remove_file(self.shm_path());
        }
    }
}
