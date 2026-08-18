use std::{fs, path::PathBuf};

use diesel::{
    QueryableByName, RunQueryDsl,
    sql_types::{BigInt, Nullable, Text},
};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    application::provisioning::{
        ProvisioningError, ProvisioningWindow, ProvisioningWindowState, close_window,
        close_window_with_ids, open_window, open_window_with_ids, read_window,
    },
    audit::{AuditEventId, CorrelationId},
    db::{
        Database, DatabaseConfig,
        tests::{test_data_version, test_observer},
    },
};

#[tokio::test]
async fn operator_open_close_open_cycle_advances_revision_and_writes_exact_audits()
-> Result<(), TestFailure> {
    let fixture = DatabaseFixture::new();
    let database = fixture.connect().await?;
    let initial = read_window(&database)
        .await
        .map_err(|_| TestFailure::WindowWasNotReadable)?;
    if initial
        != (ProvisioningWindow {
            state: ProvisioningWindowState::Closed,
            revision: 0,
        })
    {
        return Err(TestFailure::OperatorCycleChanged);
    }

    let correlation_ids = [Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()];
    let opened = open_window(&database, CorrelationId::from_uuid(correlation_ids[0]))
        .await
        .map_err(|_| TestFailure::OperatorMutationFailed)?;
    let closed = close_window(&database, CorrelationId::from_uuid(correlation_ids[1]))
        .await
        .map_err(|_| TestFailure::OperatorMutationFailed)?;
    let reopened = open_window(&database, CorrelationId::from_uuid(correlation_ids[2]))
        .await
        .map_err(|_| TestFailure::OperatorMutationFailed)?;
    if opened
        != (ProvisioningWindow {
            state: ProvisioningWindowState::Open,
            revision: 1,
        })
        || closed
            != (ProvisioningWindow {
                state: ProvisioningWindowState::Closed,
                revision: 2,
            })
        || reopened
            != (ProvisioningWindow {
                state: ProvisioningWindowState::Open,
                revision: 3,
            })
        || read_window(&database)
            .await
            .map_err(|_| TestFailure::WindowWasNotReadable)?
            != reopened
    {
        return Err(TestFailure::OperatorCycleChanged);
    }

    let evidence = persistence_snapshot(&database).await?;
    if evidence.window.state != "open"
        || evidence.window.revision != 3
        || evidence.audit_count != 4
        || evidence.audits.len() != 3
        || evidence.window.last_audit_event_id.as_deref()
            != evidence
                .audits
                .last()
                .map(|audit| audit.audit_event_id.as_str())
    {
        return Err(TestFailure::OperatorCycleChanged);
    }
    for (index, expected) in [
        (
            "open_provisioning_window",
            0,
            1,
            correlation_ids[0].to_string(),
        ),
        (
            "close_provisioning_window",
            1,
            2,
            correlation_ids[1].to_string(),
        ),
        (
            "open_provisioning_window",
            2,
            3,
            correlation_ids[2].to_string(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        verify_operator_audit(
            &evidence.audits[index],
            expected.0,
            "succeeded",
            "operator_requested",
            expected.1,
            expected.2,
            &expected.3,
        )?;
    }
    Ok(())
}

#[tokio::test]
async fn repeated_operator_targets_audit_noop_without_changing_window_facts()
-> Result<(), TestFailure> {
    let fixture = DatabaseFixture::new();
    let database = fixture.connect().await?;
    let mut observer =
        test_observer(&fixture.path).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;

    let before_closed_noop = persistence_snapshot(&database).await?;
    let version_before_closed_noop =
        test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let close_correlation = Uuid::now_v7();
    let closed = close_window(&database, CorrelationId::from_uuid(close_correlation))
        .await
        .map_err(|_| TestFailure::OperatorMutationFailed)?;
    let after_closed_noop = persistence_snapshot(&database).await?;
    let version_after_closed_noop =
        test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    if closed.state != ProvisioningWindowState::Closed
        || closed.revision != 0
        || after_closed_noop.window != before_closed_noop.window
        || after_closed_noop.audits.len() != before_closed_noop.audits.len() + 1
        || version_after_closed_noop == version_before_closed_noop
    {
        return Err(TestFailure::NoopChangedWindowFacts);
    }
    verify_operator_audit(
        after_closed_noop
            .audits
            .last()
            .ok_or(TestFailure::AuditWasNotReadable)?,
        "close_provisioning_window",
        "noop",
        "target_already_satisfied",
        0,
        0,
        &close_correlation.to_string(),
    )?;

    open_window(&database, CorrelationId::from_uuid(Uuid::now_v7()))
        .await
        .map_err(|_| TestFailure::OperatorMutationFailed)?;
    let before_open_noop = persistence_snapshot(&database).await?;
    let version_before_open_noop =
        test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let open_correlation = Uuid::now_v7();
    let opened = open_window(&database, CorrelationId::from_uuid(open_correlation))
        .await
        .map_err(|_| TestFailure::OperatorMutationFailed)?;
    let after_open_noop = persistence_snapshot(&database).await?;
    let version_after_open_noop =
        test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    if opened.state != ProvisioningWindowState::Open
        || opened.revision != 1
        || after_open_noop.window != before_open_noop.window
        || after_open_noop.audits.len() != before_open_noop.audits.len() + 1
        || version_after_open_noop == version_before_open_noop
    {
        return Err(TestFailure::NoopChangedWindowFacts);
    }
    verify_operator_audit(
        after_open_noop
            .audits
            .last()
            .ok_or(TestFailure::AuditWasNotReadable)?,
        "open_provisioning_window",
        "noop",
        "target_already_satisfied",
        1,
        1,
        &open_correlation.to_string(),
    )?;

    close_window(&database, CorrelationId::from_uuid(Uuid::now_v7()))
        .await
        .map_err(|_| TestFailure::OperatorMutationFailed)?;
    let before_second_close = persistence_snapshot(&database).await?;
    let close_again_correlation = Uuid::now_v7();
    close_window(&database, CorrelationId::from_uuid(close_again_correlation))
        .await
        .map_err(|_| TestFailure::OperatorMutationFailed)?;
    let after_second_close = persistence_snapshot(&database).await?;
    if after_second_close.window != before_second_close.window
        || after_second_close.audits.len() != before_second_close.audits.len() + 1
    {
        return Err(TestFailure::NoopChangedWindowFacts);
    }
    verify_operator_audit(
        after_second_close
            .audits
            .last()
            .ok_or(TestFailure::AuditWasNotReadable)?,
        "close_provisioning_window",
        "noop",
        "target_already_satisfied",
        2,
        2,
        &close_again_correlation.to_string(),
    )
}

#[tokio::test]
async fn duplicate_operator_audit_ids_leave_state_revision_and_pointer_unchanged()
-> Result<(), TestFailure> {
    let fixture = DatabaseFixture::new();
    let database = fixture.connect().await?;
    let duplicate_open_id = Uuid::now_v7();
    reserve_audit_id(&database, duplicate_open_id).await?;
    let before_open = persistence_snapshot(&database).await?;
    let mut observer =
        test_observer(&fixture.path).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let version_before_open =
        test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let open_result = open_window_with_ids(
        &database,
        CorrelationId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(duplicate_open_id),
    )
    .await;
    if open_result != Err(ProvisioningError::PersistenceFailed)
        || persistence_snapshot(&database).await? != before_open
        || test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?
            != version_before_open
    {
        return Err(TestFailure::OperatorAuditFailureDidNotRollBack);
    }

    open_window(&database, CorrelationId::from_uuid(Uuid::now_v7()))
        .await
        .map_err(|_| TestFailure::OperatorMutationFailed)?;
    let duplicate_close_id = Uuid::now_v7();
    reserve_audit_id(&database, duplicate_close_id).await?;
    let before_close = persistence_snapshot(&database).await?;
    let close_result = close_window_with_ids(
        &database,
        CorrelationId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(duplicate_close_id),
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await;
    if close_result != Err(ProvisioningError::PersistenceFailed)
        || persistence_snapshot(&database).await? != before_close
    {
        return Err(TestFailure::OperatorAuditFailureDidNotRollBack);
    }
    Ok(())
}

#[tokio::test]
async fn cas_failure_rolls_back_enrollment_expiry_and_both_audits() -> Result<(), TestFailure> {
    let fixture = DatabaseFixture::new();
    let database = Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, true))
        .await
        .map_err(|_| TestFailure::DatabaseCreationFailed)?;
    let opening_audit_id = Uuid::now_v7();
    seed_open_window(&database, opening_audit_id).await?;
    database
        .test_write(move |connection| {
            diesel::sql_query(
                "INSERT INTO enrollment_requests \
                 (enrollment_request_id, machine_hardware_id, hardware_identity_quality, \
                  gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, \
                  source_ip, state, created_at) VALUES \
                 ('cas-rollback-request', 'cas-rollback-machine', 'strong', x'01', \
                  zeroblob(32), 'test-client', 1, '192.0.2.1', 'pending', \
                  '2026-08-17T00:00:00.000Z')",
            )
            .execute(connection)
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
            diesel::sql_query(
                "CREATE TRIGGER ignore_provisioning_window_cas \
                 BEFORE UPDATE ON provisioning_window \
                 BEGIN SELECT RAISE(IGNORE); END",
            )
            .execute(connection)
            .map(|_| ())
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)
        })
        .await
        .map_err(|_| TestFailure::DieselInteractionFailed)??;
    let before = persistence_snapshot(&database).await?;

    let result = close_window(&database, CorrelationId::from_uuid(Uuid::now_v7())).await;
    let after = persistence_snapshot(&database).await?;
    let request_state = database
        .test_read(|connection| {
            diesel::sql_query(
                "SELECT state AS value FROM enrollment_requests \
                 WHERE enrollment_request_id = 'cas-rollback-request'",
            )
            .get_result::<TextRow>(connection)
            .map(|row| row.value)
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)
        })
        .await
        .map_err(|_| TestFailure::DieselInteractionFailed)??;
    if result != Err(ProvisioningError::PersistenceFailed)
        || after != before
        || request_state != "pending"
    {
        return Err(TestFailure::CompareAndSwapRollbackFailed);
    }
    Ok(())
}

async fn persistence_snapshot(database: &Database) -> Result<PersistenceSnapshot, TestFailure> {
    database
        .test_read(|connection| {
            let window = diesel::sql_query(
                "SELECT state, revision, last_audit_event_id \
                 FROM provisioning_window WHERE singleton = 1",
            )
            .get_result::<WindowSnapshot>(connection)
            .map_err(|_| TestFailure::WindowWasNotReadable)?;
            let audit_count = diesel::sql_query("SELECT COUNT(*) AS value FROM audit_events")
                .get_result::<CountRow>(connection)
                .map_err(|_| TestFailure::AuditWasNotReadable)?
                .value;
            let audits = diesel::sql_query(
                "SELECT audit_event_id, actor, action_kind, resource_type, resource_id, \
                 result, reason_code, correlation_id, group_correlation_id, \
                 redacted_detail_json FROM audit_events \
                 WHERE actor = 'operator:self' AND resource_type = 'provisioning_window' \
                 ORDER BY rowid",
            )
            .load::<OperatorAuditRow>(connection)
            .map_err(|_| TestFailure::AuditWasNotReadable)?;
            Ok(PersistenceSnapshot {
                window,
                audit_count,
                audits,
            })
        })
        .await
        .map_err(|_| TestFailure::DieselInteractionFailed)?
}

async fn reserve_audit_id(database: &Database, audit_event_id: Uuid) -> Result<(), TestFailure> {
    let audit_event_id = audit_event_id.to_string();
    let correlation_id = Uuid::now_v7().to_string();
    database
        .test_write(move |connection| {
            diesel::sql_query(
                "INSERT INTO audit_events (audit_event_id, occurred_at, actor, action_kind, \
                 resource_type, resource_id, result, reason_code, correlation_id, \
                 group_correlation_id, redacted_detail_json) VALUES (?, \
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'system:test', 'reserved_test_audit', \
                 'test', NULL, 'succeeded', NULL, ?, NULL, '{}')",
            )
            .bind::<Text, _>(&audit_event_id)
            .bind::<Text, _>(&correlation_id)
            .execute(connection)
            .map(|_| ())
            .map_err(|_| TestFailure::AuditFixtureInsertFailed)
        })
        .await
        .map_err(|_| TestFailure::DieselInteractionFailed)?
}

#[allow(clippy::too_many_arguments)]
fn verify_operator_audit(
    audit: &OperatorAuditRow,
    action_kind: &str,
    result: &str,
    reason_code: &str,
    previous_revision: i64,
    new_revision: i64,
    correlation_id: &str,
) -> Result<(), TestFailure> {
    let expected_detail =
        format!(r#"{{"previous_revision":{previous_revision},"new_revision":{new_revision}}}"#);
    if audit.actor != "operator:self"
        || audit.action_kind != action_kind
        || audit.resource_type != "provisioning_window"
        || audit.resource_id.is_some()
        || audit.result != result
        || audit.reason_code.as_deref() != Some(reason_code)
        || audit.correlation_id != correlation_id
        || audit.group_correlation_id.is_some()
        || audit.redacted_detail_json != expected_detail
    {
        return Err(TestFailure::OperatorAuditChanged);
    }
    Ok(())
}

async fn seed_open_window(database: &Database, audit_event_id: Uuid) -> Result<(), TestFailure> {
    let audit_event_id = audit_event_id.to_string();
    let correlation_id = Uuid::now_v7().to_string();
    database
        .test_write(move |connection| {
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

#[derive(Debug, PartialEq, Eq)]
struct PersistenceSnapshot {
    window: WindowSnapshot,
    audit_count: i64,
    audits: Vec<OperatorAuditRow>,
}

#[derive(Debug, PartialEq, Eq, QueryableByName)]
struct WindowSnapshot {
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = BigInt)]
    revision: i64,
    #[diesel(sql_type = Nullable<Text>)]
    last_audit_event_id: Option<String>,
}

#[derive(Debug, PartialEq, Eq, QueryableByName)]
struct OperatorAuditRow {
    #[diesel(sql_type = Text)]
    audit_event_id: String,
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Text)]
    action_kind: String,
    #[diesel(sql_type = Text)]
    resource_type: String,
    #[diesel(sql_type = Nullable<Text>)]
    resource_id: Option<String>,
    #[diesel(sql_type = Text)]
    result: String,
    #[diesel(sql_type = Nullable<Text>)]
    reason_code: Option<String>,
    #[diesel(sql_type = Text)]
    correlation_id: String,
    #[diesel(sql_type = Nullable<Text>)]
    group_correlation_id: Option<String>,
    #[diesel(sql_type = Text)]
    redacted_detail_json: String,
}

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

#[derive(QueryableByName)]
struct TextRow {
    #[diesel(sql_type = Text)]
    value: String,
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
    #[snafu(display("the provisioning window was not readable"))]
    WindowWasNotReadable,
    #[snafu(display("the audit evidence was not readable"))]
    AuditWasNotReadable,
    #[snafu(display("a provisioning CAS failure did not roll back expiry and audits"))]
    CompareAndSwapRollbackFailed,
    #[snafu(display("the operator provisioning-window cycle changed"))]
    OperatorCycleChanged,
    #[snafu(display("the operator provisioning-window mutation failed"))]
    OperatorMutationFailed,
    #[snafu(display("the operator provisioning-window audit changed"))]
    OperatorAuditChanged,
    #[snafu(display("a repeat-safe action changed provisioning-window facts"))]
    NoopChangedWindowFacts,
    #[snafu(display("provisioning-window database evidence could not be read"))]
    DatabaseEvidenceFailed,
    #[snafu(display("an operator audit failure did not roll back window facts"))]
    OperatorAuditFailureDidNotRollBack,
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

    async fn connect(&self) -> Result<Database, TestFailure> {
        Database::connect_and_migrate(&DatabaseConfig::new(&self.path, true))
            .await
            .map_err(|_| TestFailure::DatabaseCreationFailed)
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
