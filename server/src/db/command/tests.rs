use std::{fs, path::PathBuf};

use diesel::{QueryableByName, RunQueryDsl, sql_types::BigInt};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    application::command::REQUEST_FINGERPRINT_VERSION,
    audit::{AuditEvent, AuditEventId, CorrelationId},
    db::{Database, DatabaseConfig},
};

use super::{InsertCommandOutcome, NewCommand, find_command, insert_command_with_created_audit};

#[tokio::test]
async fn insert_command_and_created_audit_are_atomic_and_replay_facts_are_readable()
-> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let command_id = Uuid::now_v7();
    seed_device(&fixture.database).await?;
    let inserted = insert_command_with_created_audit(
        &fixture.database,
        new_command(command_id),
        created_event(command_id),
    )
    .await
    .map_err(|_| TestFailure::CommandInsertFailed)?;
    if inserted != InsertCommandOutcome::Inserted {
        return Err(TestFailure::CommandInsertFailed);
    }

    let persisted = find_command(&fixture.database, &command_id.to_string())
        .await
        .map_err(|_| TestFailure::CommandReadFailed)?
        .ok_or(TestFailure::CommandReadFailed)?;
    if persisted.request_fingerprint_version != REQUEST_FINGERPRINT_VERSION
        || persisted.request_fingerprint_sha256 != vec![0x5a; 32]
    {
        return Err(TestFailure::CommandReadFailed);
    }

    let duplicate = insert_command_with_created_audit(
        &fixture.database,
        new_command(command_id),
        created_event(command_id),
    )
    .await;
    if !matches!(duplicate, Ok(InsertCommandOutcome::CommandIdExists)) {
        return Err(TestFailure::DuplicateCommandWasAccepted);
    }
    let counts = command_audit_counts(&fixture.database).await?;
    if counts.command_count != 1 || counts.audit_count != 1 {
        return Err(TestFailure::CommandAuditWasNotAtomic);
    }
    Ok(())
}

fn new_command(command_id: Uuid) -> NewCommand {
    NewCommand {
        command_id: command_id.to_string(),
        device_pk: "01900000-0000-7000-8000-000000000201".to_owned(),
        kind: "lock_session",
        request_fingerprint_version: REQUEST_FINGERPRINT_VERSION,
        request_fingerprint_sha256: vec![0x5a; 32],
        group_correlation_id: None,
        payload_version: 1,
        frozen_payload_json:
            r#"{"requested_lock_epoch":2,"target":{"session_epoch":1,"session_instance_id":"session-a"}}"#
                .to_owned(),
    }
}

fn created_event(command_id: Uuid) -> AuditEvent {
    AuditEvent::command_created(
        AuditEventId::from_uuid(Uuid::now_v7()),
        CorrelationId::from_uuid(Uuid::now_v7()),
        command_id,
        None,
        "lock_session",
        1,
        REQUEST_FINGERPRINT_VERSION,
    )
}

async fn seed_device(database: &Database) -> Result<(), TestFailure> {
    database
        .interact(|connection| {
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

async fn command_audit_counts(database: &Database) -> Result<CountEvidence, TestFailure> {
    database
        .interact(|connection| {
            diesel::sql_query(
                "SELECT (SELECT COUNT(*) FROM commands) AS command_count, \
                 (SELECT COUNT(*) FROM audit_events WHERE action_kind = 'command_create') \
                 AS audit_count",
            )
            .get_result::<CountEvidence>(connection)
        })
        .await
        .map_err(|_| TestFailure::EvidenceFailed)?
        .map_err(|_| TestFailure::EvidenceFailed)
}

#[derive(QueryableByName)]
struct CountEvidence {
    #[diesel(sql_type = BigInt)]
    command_count: i64,
    #[diesel(sql_type = BigInt)]
    audit_count: i64,
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
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(format!("{}-wal", self.path.display()));
        let _ = fs::remove_file(format!("{}-shm", self.path.display()));
    }
}

#[derive(Debug, Snafu)]
enum TestFailure {
    #[snafu(display("the command database fixture failed"))]
    FixtureFailed,
    #[snafu(display("the command could not be inserted"))]
    CommandInsertFailed,
    #[snafu(display("the command replay facts could not be read"))]
    CommandReadFailed,
    #[snafu(display("a duplicate command was accepted"))]
    DuplicateCommandWasAccepted,
    #[snafu(display("the command and created audit were not atomic"))]
    CommandAuditWasNotAtomic,
    #[snafu(display("command database evidence could not be read"))]
    EvidenceFailed,
}
