use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use diesel::{
    QueryableByName, RunQueryDsl,
    connection::SimpleConnection,
    sql_types::{BigInt, Nullable, Text},
};
use natsume_device_protocol::generated::{CommandState, CommandStatus};
use serde_json::value::RawValue;
use snafu::Snafu;
use tokio::time::timeout;
use uuid::Uuid;

use crate::{
    application::command::{
        CommandError, CommandId, CommandKind, CommandOutcome, CommandRequestInput,
        CommandStatusWriteOutcome, DeviceCommandDispatchNotifier, list_dispatchable_commands,
        put_command, writeback_command_status,
    },
    audit::CorrelationId,
    db::{Database, DatabaseConfig, tests as db_tests},
};

const DEVICE_ID: &str = "01900000-0000-7000-8000-000000000201";
const FOREIGN_DEVICE_ID: &str = "01900000-0000-7000-8000-000000000299";
const COMMAND_ID: &str = "01900000-0000-7000-8000-000000000203";
const SECOND_COMMAND_ID: &str = "01900000-0000-7000-8000-000000000204";
const LOCK_PAYLOAD: &str =
    r#"{"target":{"session_instance_id":"session-a","session_epoch":1},"requested_lock_epoch":2}"#;

#[test]
fn invalid_persisted_kind_is_a_persistence_failure() {
    assert_eq!(
        CommandKind::parse_persisted("outside_frozen_vocabulary"),
        Err(CommandError::PersistenceFailed)
    );
}

#[tokio::test]
async fn created_command_and_audit_are_linked_atomically_and_replay_is_zero_write()
-> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    seed_device(&fixture.database).await?;
    let notifier = CountingNotifier::default();

    let created = create_command(&fixture.database, COMMAND_ID, None, &notifier).await?;
    if created != CommandOutcome::Created || notifier.count() != 1 {
        return Err(TestFailure::CommandCreateFailed);
    }
    let initial_evidence = created_evidence(&fixture.database).await?;
    if initial_evidence.commands != 1
        || initial_evidence.created_audits != 1
        || initial_evidence.linked != 1
    {
        return Err(TestFailure::CreatedAuditLinkageChanged);
    }

    let mut observer =
        db_tests::test_observer(&fixture.path).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let before_replay = db_tests::test_data_version(&mut observer)
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let replayed = create_command(&fixture.database, COMMAND_ID, None, &notifier).await?;
    let after_replay = db_tests::test_data_version(&mut observer)
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    if replayed != CommandOutcome::Replayed
        || before_replay != after_replay
        || notifier.count() != 1
        || created_evidence(&fixture.database).await? != initial_evidence
    {
        return Err(TestFailure::ReplayWroteData);
    }

    let conflict = create_command(
        &fixture.database,
        COMMAND_ID,
        Some("operator_requested"),
        &notifier,
    )
    .await;
    if conflict != Err(CommandError::RequestConflict) {
        return Err(TestFailure::ConflictClassificationChanged);
    }
    let after_conflict = created_evidence(&fixture.database).await?;
    if after_conflict.commands != 1
        || after_conflict.created_audits != 2
        || after_conflict.linked != 1
        || notifier.count() != 1
    {
        return Err(TestFailure::ConflictAuditChanged);
    }
    Ok(())
}

#[tokio::test]
async fn created_audit_failure_leaves_no_command_and_sends_no_notification()
-> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    seed_device(&fixture.database).await?;
    install_created_audit_failure(&fixture.database).await?;
    let notifier = CountingNotifier::default();

    let result = create_command(&fixture.database, COMMAND_ID, None, &notifier).await;
    if result != Err(CommandError::PersistenceFailed)
        || notifier.count() != 0
        || created_evidence(&fixture.database).await? != CreatedEvidence::default()
    {
        return Err(TestFailure::CreatedAuditFailureWasNotAtomic);
    }
    Ok(())
}

#[tokio::test]
async fn concurrent_same_id_requests_serialize_to_created_and_replay_or_conflict()
-> Result<(), TestFailure> {
    let replay_fixture = TestDatabase::new().await?;
    seed_device(&replay_fixture.database).await?;
    let replay_notifier = CountingNotifier::default();
    let (first, second) =
        concurrent_puts(&replay_fixture.database, &replay_notifier, None, None).await?;
    if !matches!(
        (first, second),
        (Ok(CommandOutcome::Created), Ok(CommandOutcome::Replayed))
            | (Ok(CommandOutcome::Replayed), Ok(CommandOutcome::Created))
    ) || replay_notifier.count() != 1
        || created_evidence(&replay_fixture.database).await?
            != (CreatedEvidence {
                commands: 1,
                created_audits: 1,
                linked: 1,
            })
    {
        return Err(TestFailure::ConcurrentReplayClassificationChanged);
    }

    let conflict_fixture = TestDatabase::new().await?;
    seed_device(&conflict_fixture.database).await?;
    let conflict_notifier = CountingNotifier::default();
    let (first, second) = concurrent_puts(
        &conflict_fixture.database,
        &conflict_notifier,
        None,
        Some("operator_requested"),
    )
    .await?;
    if !matches!(
        (first, second),
        (
            Ok(CommandOutcome::Created),
            Err(CommandError::RequestConflict)
        ) | (
            Err(CommandError::RequestConflict),
            Ok(CommandOutcome::Created)
        )
    ) || conflict_notifier.count() != 1
        || created_evidence(&conflict_fixture.database).await?
            != (CreatedEvidence {
                commands: 1,
                created_audits: 2,
                linked: 1,
            })
    {
        return Err(TestFailure::ConcurrentConflictClassificationChanged);
    }
    Ok(())
}

#[tokio::test]
async fn status_writeback_is_monotonic_owned_terminal_and_exactly_audited()
-> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    seed_device(&fixture.database).await?;
    let notifier = CountingNotifier::default();
    let _created = create_command(&fixture.database, COMMAND_ID, None, &notifier).await?;

    let dispatchable = list_dispatchable_commands(&fixture.database, DEVICE_ID)
        .await
        .map_err(|_| TestFailure::CommandReadFailed)?;
    if dispatchable.len() != 1
        || dispatchable[0].command_id != COMMAND_ID
        || dispatchable[0].kind != CommandKind::LockSession
        || dispatchable[0].payload_version != 1
        || dispatchable[0].deadline_at.is_some()
    {
        return Err(TestFailure::CommandReadFailed);
    }

    exercise_monotonic_status_transitions(&fixture.database).await?;
    let foreign = write_status(
        &fixture.database,
        FOREIGN_DEVICE_ID,
        COMMAND_ID,
        CommandState::Running,
        "",
    )
    .await?;
    let unknown = write_status(
        &fixture.database,
        DEVICE_ID,
        SECOND_COMMAND_ID,
        CommandState::Running,
        "",
    )
    .await?;
    if foreign != CommandStatusWriteOutcome::IgnoredForeignCommand
        || unknown != CommandStatusWriteOutcome::IgnoredUnknownCommand
    {
        return Err(TestFailure::StatusClassificationChanged);
    }

    let evidence = status_evidence(&fixture.database).await?;
    if evidence.state != "succeeded"
        || evidence.terminal_error_code.is_some()
        || evidence.audit_count != 1
        || evidence.actor.as_deref() != Some("device:control")
        || evidence.action_kind.as_deref() != Some("command_terminal")
        || evidence.result.as_deref() != Some("succeeded")
        || evidence.reason_code.as_deref() != Some("device_reported")
        || evidence.redacted_detail_json.as_deref()
            != Some(r#"{"kind":"lock_session","terminal_state":"succeeded"}"#)
        || !list_dispatchable_commands(&fixture.database, DEVICE_ID)
            .await
            .map_err(|_| TestFailure::CommandReadFailed)?
            .is_empty()
    {
        return Err(TestFailure::TerminalEvidenceChanged);
    }
    Ok(())
}

async fn exercise_monotonic_status_transitions(database: &Database) -> Result<(), TestFailure> {
    let received =
        write_status(database, DEVICE_ID, COMMAND_ID, CommandState::Received, "").await?;
    let running = write_status(database, DEVICE_ID, COMMAND_ID, CommandState::Running, "").await?;
    let backwards =
        write_status(database, DEVICE_ID, COMMAND_ID, CommandState::Received, "").await?;
    let terminal =
        write_status(database, DEVICE_ID, COMMAND_ID, CommandState::Succeeded, "").await?;
    let duplicate =
        write_status(database, DEVICE_ID, COMMAND_ID, CommandState::Succeeded, "").await?;
    let overwrite = write_status(
        database,
        DEVICE_ID,
        COMMAND_ID,
        CommandState::Failed,
        "HOME_OPERATION_FAILED",
    )
    .await?;
    if received != CommandStatusWriteOutcome::UpdatedNonterminal
        || running != CommandStatusWriteOutcome::UpdatedNonterminal
        || backwards != CommandStatusWriteOutcome::IgnoredRegression
        || terminal != CommandStatusWriteOutcome::UpdatedTerminal
        || duplicate != CommandStatusWriteOutcome::IgnoredTransition
        || overwrite != CommandStatusWriteOutcome::IgnoredRegression
    {
        return Err(TestFailure::StatusClassificationChanged);
    }
    Ok(())
}

#[tokio::test]
async fn terminal_audit_failure_rolls_command_state_back_to_running() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    seed_device(&fixture.database).await?;
    let notifier = CountingNotifier::default();
    let _created = create_command(&fixture.database, COMMAND_ID, None, &notifier).await?;
    let _running = write_status(
        &fixture.database,
        DEVICE_ID,
        COMMAND_ID,
        CommandState::Running,
        "",
    )
    .await?;
    install_terminal_audit_failure(&fixture.database).await?;

    let result = write_status(
        &fixture.database,
        DEVICE_ID,
        COMMAND_ID,
        CommandState::Failed,
        "HOME_OPERATION_FAILED",
    )
    .await;
    let evidence = status_evidence(&fixture.database).await?;
    if !matches!(result, Err(TestFailure::StatusWritebackFailed))
        || evidence.state != "running"
        || evidence.terminal_error_code.is_some()
        || evidence.audit_count != 0
    {
        return Err(TestFailure::TerminalAuditFailureWasNotAtomic);
    }
    Ok(())
}

async fn concurrent_puts(
    database: &Database,
    notifier: &CountingNotifier,
    first_reason: Option<&str>,
    second_reason: Option<&str>,
) -> Result<
    (
        Result<CommandOutcome, CommandError>,
        Result<CommandOutcome, CommandError>,
    ),
    TestFailure,
> {
    let first_database = database.clone();
    let second_database = database.clone();
    let first_notifier = notifier.clone();
    let second_notifier = notifier.clone();
    let first_reason = first_reason.map(str::to_owned);
    let second_reason = second_reason.map(str::to_owned);
    timeout(Duration::from_secs(10), async move {
        tokio::join!(
            async move {
                put_command(
                    &first_database,
                    &command_id(COMMAND_ID)?,
                    command_request(first_reason.as_deref()),
                    CorrelationId::from_uuid(Uuid::now_v7()),
                    &first_notifier,
                )
                .await
            },
            async move {
                put_command(
                    &second_database,
                    &command_id(COMMAND_ID)?,
                    command_request(second_reason.as_deref()),
                    CorrelationId::from_uuid(Uuid::now_v7()),
                    &second_notifier,
                )
                .await
            }
        )
    })
    .await
    .map_err(|_| TestFailure::ConcurrentPutTimedOut)
}

async fn create_command(
    database: &Database,
    command_id_text: &str,
    reason_code: Option<&str>,
    notifier: &CountingNotifier,
) -> Result<CommandOutcome, CommandError> {
    put_command(
        database,
        &command_id(command_id_text)?,
        command_request(reason_code),
        CorrelationId::from_uuid(Uuid::now_v7()),
        notifier,
    )
    .await
}

fn command_id(value: &str) -> Result<CommandId, CommandError> {
    CommandId::parse(value)
}

fn command_request(reason_code: Option<&str>) -> CommandRequestInput {
    CommandRequestInput {
        device_id: DEVICE_ID.to_owned(),
        kind: "lock_session".to_owned(),
        payload_version: 1,
        payload: raw(LOCK_PAYLOAD),
        reason_code: reason_code.map(str::to_owned),
        group_correlation_id: None,
    }
}

fn raw(value: &str) -> Box<RawValue> {
    match serde_json::from_str(value) {
        Ok(value) => value,
        Err(error) => {
            drop(error);
            panic!("the command test payload was invalid");
        }
    }
}

async fn write_status(
    database: &Database,
    device_pk: &str,
    command_id: &str,
    state: CommandState,
    stable_error_code: &str,
) -> Result<CommandStatusWriteOutcome, TestFailure> {
    writeback_command_status(
        database,
        device_pk,
        &CommandStatus {
            command_id: command_id.to_owned(),
            state: state as i32,
            stable_error_code: stable_error_code.to_owned(),
        },
    )
    .await
    .map_err(|_| TestFailure::StatusWritebackFailed)
}

async fn seed_device(database: &Database) -> Result<(), TestFailure> {
    database
        .test_write(|connection| {
            diesel::sql_query(
                "INSERT INTO devices VALUES \
                 ('01900000-0000-7000-8000-000000000201', 'command-db-hardware', \
                  'strong', 'enrolled')",
            )
            .execute(connection)
        })
        .await
        .map_err(|_| TestFailure::FixtureFailed)?
        .map(|_| ())
        .map_err(|_| TestFailure::FixtureFailed)
}

async fn install_created_audit_failure(database: &Database) -> Result<(), TestFailure> {
    install_audit_failure(
        database,
        "CREATE TRIGGER fail_created_command_audit BEFORE INSERT ON audit_events \
         WHEN NEW.action_kind = 'command_create' AND NEW.result = 'succeeded' \
         BEGIN SELECT RAISE(ABORT, 'created audit failure canary'); END;",
    )
    .await
}

async fn install_terminal_audit_failure(database: &Database) -> Result<(), TestFailure> {
    install_audit_failure(
        database,
        "CREATE TRIGGER fail_terminal_command_audit BEFORE INSERT ON audit_events \
         WHEN NEW.action_kind = 'command_terminal' \
         BEGIN SELECT RAISE(ABORT, 'terminal audit failure canary'); END;",
    )
    .await
}

async fn install_audit_failure(
    database: &Database,
    statement: &'static str,
) -> Result<(), TestFailure> {
    database
        .test_write(move |connection| connection.batch_execute(statement))
        .await
        .map_err(|_| TestFailure::FixtureFailed)?
        .map_err(|_| TestFailure::FixtureFailed)
}

async fn created_evidence(database: &Database) -> Result<CreatedEvidence, TestFailure> {
    database
        .test_read(|connection| {
            diesel::sql_query(
                "SELECT (SELECT COUNT(*) FROM commands) AS commands, \
                 (SELECT COUNT(*) FROM audit_events WHERE action_kind = 'command_create') \
                 AS created_audits, \
                 (SELECT COUNT(*) FROM commands c JOIN audit_events a \
                  ON a.audit_event_id = c.created_audit_event_id \
                  WHERE a.action_kind = 'command_create' AND a.result = 'succeeded') \
                 AS linked",
            )
            .get_result::<CreatedEvidence>(connection)
        })
        .await
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)
}

async fn status_evidence(database: &Database) -> Result<StatusEvidence, TestFailure> {
    database
        .test_read(|connection| {
            diesel::sql_query(
                "SELECT c.state AS state, c.terminal_error_code AS terminal_error_code, \
                 (SELECT COUNT(*) FROM audit_events WHERE action_kind = 'command_terminal') \
                 AS audit_count, \
                 (SELECT actor FROM audit_events WHERE action_kind = 'command_terminal') AS actor, \
                 (SELECT action_kind FROM audit_events WHERE action_kind = 'command_terminal') \
                 AS action_kind, \
                 (SELECT result FROM audit_events WHERE action_kind = 'command_terminal') \
                 AS result, \
                 (SELECT reason_code FROM audit_events WHERE action_kind = 'command_terminal') \
                 AS reason_code, \
                 (SELECT redacted_detail_json FROM audit_events \
                  WHERE action_kind = 'command_terminal') AS redacted_detail_json \
                 FROM commands c LIMIT 1",
            )
            .get_result::<StatusEvidence>(connection)
        })
        .await
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)
}

#[derive(Debug, Default, PartialEq, Eq, QueryableByName)]
struct CreatedEvidence {
    #[diesel(sql_type = BigInt)]
    commands: i64,
    #[diesel(sql_type = BigInt)]
    created_audits: i64,
    #[diesel(sql_type = BigInt)]
    linked: i64,
}

#[derive(QueryableByName)]
struct StatusEvidence {
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = Nullable<Text>)]
    terminal_error_code: Option<String>,
    #[diesel(sql_type = BigInt)]
    audit_count: i64,
    #[diesel(sql_type = Nullable<Text>)]
    actor: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    action_kind: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    result: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    reason_code: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    redacted_detail_json: Option<String>,
}

#[derive(Clone, Default)]
struct CountingNotifier {
    notifications: Arc<AtomicUsize>,
}

impl CountingNotifier {
    fn count(&self) -> usize {
        self.notifications.load(Ordering::SeqCst)
    }
}

impl DeviceCommandDispatchNotifier for CountingNotifier {
    fn notify_command_dispatch(&self, _device_pk: &str) {
        self.notifications.fetch_add(1, Ordering::SeqCst);
    }
}

struct TestDatabase {
    database: Database,
    path: PathBuf,
}

impl TestDatabase {
    async fn new() -> Result<Self, TestFailure> {
        let path = std::env::temp_dir().join(format!(
            "natsume-command-db-test-{}.sqlite3",
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
        remove_database_files(&self.path);
    }
}

fn remove_database_files(path: &Path) {
    let _remove_result = fs::remove_file(path);
    let _remove_wal_result = fs::remove_file(format!("{}-wal", path.display()));
    let _remove_shm_result = fs::remove_file(format!("{}-shm", path.display()));
}

#[derive(Debug, Snafu)]
enum TestFailure {
    #[snafu(display("the command database fixture failed"))]
    FixtureFailed,
    #[snafu(display("the command create failed"))]
    CommandCreateFailed,
    #[snafu(display("the created command/audit linkage changed"))]
    CreatedAuditLinkageChanged,
    #[snafu(display("a replay wrote data"))]
    ReplayWroteData,
    #[snafu(display("the conflict classification changed"))]
    ConflictClassificationChanged,
    #[snafu(display("the conflict audit changed"))]
    ConflictAuditChanged,
    #[snafu(display("created-audit failure did not leave the command transaction atomic"))]
    CreatedAuditFailureWasNotAtomic,
    #[snafu(display("concurrent replay classification changed"))]
    ConcurrentReplayClassificationChanged,
    #[snafu(display("concurrent conflict classification changed"))]
    ConcurrentConflictClassificationChanged,
    #[snafu(display("concurrent command PUTs timed out"))]
    ConcurrentPutTimedOut,
    #[snafu(display("the command could not be read"))]
    CommandReadFailed,
    #[snafu(display("CommandStatus writeback failed"))]
    StatusWritebackFailed,
    #[snafu(display("CommandStatus classification changed"))]
    StatusClassificationChanged,
    #[snafu(display("terminal command evidence changed"))]
    TerminalEvidenceChanged,
    #[snafu(display("terminal-audit failure did not roll the command state back"))]
    TerminalAuditFailureWasNotAtomic,
    #[snafu(display("command database evidence could not be read"))]
    DatabaseEvidenceFailed,
}

impl From<CommandError> for TestFailure {
    fn from(_source: CommandError) -> Self {
        Self::CommandCreateFailed
    }
}
