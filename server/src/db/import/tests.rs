use std::{fs, path::PathBuf};

use diesel::{
    Connection, QueryDsl, QueryableByName, RunQueryDsl,
    connection::SimpleConnection,
    sql_types::{BigInt, Text},
    sqlite::SqliteConnection,
};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    application::import::{CandidateRowFacts, ImportError, SealedCommitRow},
    audit::{AuditEventId, CorrelationId},
    db::{
        Database, DatabaseConfig,
        schema::{
            account_mappings, accounts, audit_events, device_bindings, pending_import_candidate,
            revision_counters, seats, server_vault_records,
        },
    },
};

use super::{
    CandidateCreationRequest, CommitOutcome, CommitRequest, ImportStoreError,
    audit_preview_token_mismatch_with_ids, commit_import_with_ids,
    create_import_candidate_with_ids,
};

#[tokio::test]
async fn create_audit_failure_rolls_back_expiry_and_replacement() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let old_candidate_id = Uuid::now_v7();
    let old_payload_id = Uuid::now_v7();
    let duplicate_audit_id = Uuid::now_v7();
    seed_expired_candidate(
        &fixture.database,
        old_candidate_id,
        old_payload_id,
        duplicate_audit_id,
    )
    .await?;
    let mut observer = fixture.observer()?;
    let before = rollback_snapshot(&fixture.database).await?;
    let data_version_before = data_version(&mut observer)?;

    let request = CandidateCreationRequest {
        candidate_rows: vec![CandidateRowFacts {
            seat_code: "B-02".to_owned(),
            domjudge_username: "team-b".to_owned(),
        }],
        preview_token_hash: [0x42; 32],
        nonce: [0x24; 24],
        ciphertext: vec![0x55],
        correlation_id: CorrelationId::from_uuid(Uuid::now_v7()),
    };
    let Err(error) = create_import_candidate_with_ids(
        &fixture.database,
        request,
        AuditEventId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(duplicate_audit_id),
    )
    .await
    else {
        return Err(TestFailure::ExpectedCreateAuditFailure);
    };
    if error != ImportStoreError::AuditInsertFailed {
        return Err(TestFailure::UnexpectedStoreFailure);
    }

    let after = rollback_snapshot(&fixture.database).await?;
    let data_version_after = data_version(&mut observer)?;
    if before != after
        || data_version_before != data_version_after
        || after.candidate_id != old_candidate_id.to_string()
        || after.payload_vault_record_id != old_payload_id.to_string()
        || after.candidate_count != 1
        || after.old_payload_count != 1
        || after.expiry_audit_count != 0
        || after.audit_count != 1
    {
        return Err(TestFailure::CompoundMutationDidNotRollBack);
    }
    Ok(())
}

#[tokio::test]
async fn commit_audit_failure_rolls_back_every_business_mutation() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let candidate_id = Uuid::now_v7();
    let payload_id = Uuid::now_v7();
    let duplicate_audit_id = Uuid::now_v7();
    seed_commit_candidate(
        &fixture.database,
        candidate_id,
        payload_id,
        duplicate_audit_id,
    )
    .await?;
    let before = full_rollback_snapshot(&fixture.database).await?;
    let mut observer = fixture.observer()?;
    let data_version_before = data_version(&mut observer)?;
    let request = CommitRequest {
        candidate_id,
        expected_preview_token_hash: [0x42; 32],
        expected_payload_vault_record_id: payload_id,
        sealed_rows: vec![
            SealedCommitRow {
                seat_code: "A-01".to_owned(),
                domjudge_username: "team-a".to_owned(),
                nonce: [0x31; 24],
                ciphertext: vec![0x41, 0x42],
            },
            SealedCommitRow {
                seat_code: "C-03".to_owned(),
                domjudge_username: "team-c".to_owned(),
                nonce: [0x32; 24],
                ciphertext: vec![0x43, 0x44],
            },
        ],
        correlation_id: CorrelationId::from_uuid(Uuid::now_v7()),
    };
    let Err(error) = commit_import_with_ids(
        &fixture.database,
        request,
        AuditEventId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(duplicate_audit_id),
    )
    .await
    else {
        return Err(TestFailure::ExpectedCommitAuditFailure);
    };
    if error != ImportStoreError::AuditInsertFailed {
        return Err(TestFailure::UnexpectedStoreFailure);
    }

    let after = full_rollback_snapshot(&fixture.database).await?;
    let data_version_after = data_version(&mut observer)?;
    if before != after || data_version_before != data_version_after {
        return Err(TestFailure::CommitMutationDidNotRollBack);
    }
    Ok(())
}

#[tokio::test]
async fn token_mismatch_audit_failure_preserves_unavailable_outcome() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let candidate_id = Uuid::now_v7();
    let payload_id = Uuid::now_v7();
    let duplicate_audit_id = Uuid::now_v7();
    seed_commit_candidate(
        &fixture.database,
        candidate_id,
        payload_id,
        duplicate_audit_id,
    )
    .await?;
    let before = full_rollback_snapshot(&fixture.database).await?;
    let mut observer = fixture.observer()?;
    let data_version_before = data_version(&mut observer)?;

    audit_preview_token_mismatch_with_ids(
        &fixture.database,
        candidate_id,
        [0x42; 32],
        CorrelationId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(duplicate_audit_id),
    )
    .await
    .map_err(|_| TestFailure::RejectionAuditFailureEscaped)?;

    if full_rollback_snapshot(&fixture.database).await? != before
        || data_version(&mut observer)? != data_version_before
    {
        return Err(TestFailure::RejectedAuditFailureWroteData);
    }
    Ok(())
}

#[tokio::test]
async fn baseline_stale_audit_failure_preserves_stale_outcome() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let candidate_id = Uuid::now_v7();
    let payload_id = Uuid::now_v7();
    let duplicate_audit_id = Uuid::now_v7();
    seed_commit_candidate(
        &fixture.database,
        candidate_id,
        payload_id,
        duplicate_audit_id,
    )
    .await?;
    bump_fixture_configuration_revision(&fixture.database).await?;
    let before = full_rollback_snapshot(&fixture.database).await?;
    let mut observer = fixture.observer()?;
    let data_version_before = data_version(&mut observer)?;
    let request = CommitRequest {
        candidate_id,
        expected_preview_token_hash: [0x42; 32],
        expected_payload_vault_record_id: payload_id,
        sealed_rows: vec![SealedCommitRow {
            seat_code: "A-01".to_owned(),
            domjudge_username: "team-a".to_owned(),
            nonce: [0x33; 24],
            ciphertext: vec![0x45],
        }],
        correlation_id: CorrelationId::from_uuid(Uuid::now_v7()),
    };
    let outcome = commit_import_with_ids(
        &fixture.database,
        request,
        AuditEventId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(duplicate_audit_id),
    )
    .await
    .map_err(|_| TestFailure::RejectionAuditFailureEscaped)?;
    if !matches!(outcome, CommitOutcome::Stale)
        || full_rollback_snapshot(&fixture.database).await? != before
        || data_version(&mut observer)? != data_version_before
    {
        return Err(TestFailure::RejectedAuditFailureWroteData);
    }
    Ok(())
}

#[tokio::test]
async fn invalid_commit_stage_facts_are_persistence_classified() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let candidate_id = Uuid::now_v7();
    let payload_id = Uuid::now_v7();
    seed_commit_candidate(&fixture.database, candidate_id, payload_id, Uuid::now_v7()).await?;
    let before = full_rollback_snapshot(&fixture.database).await?;
    let Err(error) = super::commit_import(
        &fixture.database,
        candidate_id,
        [0x42; 32],
        payload_id,
        Vec::new(),
        CorrelationId::from_uuid(Uuid::now_v7()),
    )
    .await
    else {
        return Err(TestFailure::InvalidCommitFactsWereAccepted);
    };
    if error != ImportError::PersistenceFailure
        || full_rollback_snapshot(&fixture.database).await? != before
    {
        return Err(TestFailure::InvalidCommitClassificationChanged);
    }
    Ok(())
}

async fn bump_fixture_configuration_revision(database: &Database) -> Result<(), TestFailure> {
    database
        .interact(|connection| {
            diesel::sql_query(
                "UPDATE revision_counters SET configuration_revision = 1 \
                 WHERE singleton = 1",
            )
            .execute(connection)
        })
        .await
        .map_err(|_| TestFailure::FixtureFailed)?
        .map(|_| ())
        .map_err(|_| TestFailure::FixtureFailed)
}

async fn seed_commit_candidate(
    database: &Database,
    candidate_id: Uuid,
    payload_id: Uuid,
    audit_event_id: Uuid,
) -> Result<(), TestFailure> {
    let candidate_id = candidate_id.to_string();
    let payload_id = payload_id.to_string();
    let audit_event_id = audit_event_id.to_string();
    let correlation_id = Uuid::now_v7().to_string();
    let preview_hash = "42".repeat(32);
    database
        .interact(move |connection| {
            connection.batch_execute(&format!(
                "INSERT INTO server_vault_records \
                 (vault_record_id, record_type, subject_id, nonce, ciphertext) VALUES \
                 ('rollback-vault-a', 'account_credential', 'rollback-account-a', x'01', x'11'), \
                 ('rollback-vault-b', 'account_credential', 'rollback-account-b', x'02', x'12'), \
                 ('{payload_id}', 'import_payload', '{candidate_id}', x'03', x'13'); \
                 INSERT INTO seats (seat_id, seat_code) VALUES \
                 ('rollback-seat-a', 'A-01'), ('rollback-seat-b', 'B-02'); \
                 INSERT INTO accounts \
                 (account_id, domjudge_username, credential_vault_record_id, credential_revision) VALUES \
                 ('rollback-account-a', 'team-a', 'rollback-vault-a', 7), \
                 ('rollback-account-b', 'team-b', 'rollback-vault-b', 8); \
                 INSERT INTO account_mappings (seat_id, account_id) VALUES \
                 ('rollback-seat-a', 'rollback-account-a'), \
                 ('rollback-seat-b', 'rollback-account-b'); \
                 INSERT INTO devices \
                 (device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES \
                 ('rollback-device-b', 'rollback-machine-b', 'strong', 'enrolled'); \
                 INSERT INTO device_bindings (seat_id, device_pk, binding_revision) VALUES \
                 ('rollback-seat-b', 'rollback-device-b', 1); \
                 INSERT INTO pending_import_candidate \
                 (singleton, candidate_id, expires_at, baseline_configuration_revision, \
                  baseline_binding_revision, preview_token_hash, payload_vault_record_id, \
                  redacted_preview_json) VALUES \
                 (1, '{candidate_id}', \
                  strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+1800 seconds'), \
                  0, 0, x'{preview_hash}', '{payload_id}', '{{}}'); \
                 INSERT INTO audit_events \
                 (audit_event_id, occurred_at, actor, action_kind, resource_type, resource_id, \
                  result, reason_code, correlation_id, group_correlation_id, \
                  redacted_detail_json) VALUES \
                 ('{audit_event_id}', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), \
                  'system:test', 'fixture', 'import_candidate', '{candidate_id}', \
                  'succeeded', NULL, '{correlation_id}', NULL, '{{}}');"
            ))
        })
        .await
        .map_err(|_| TestFailure::FixtureFailed)?
        .map_err(|_| TestFailure::FixtureFailed)
}

async fn full_rollback_snapshot(database: &Database) -> Result<FullRollbackSnapshot, TestFailure> {
    database
        .interact(|connection| {
            let revisions = revision_counters::table
                .order(revision_counters::singleton)
                .select((
                    diesel::dsl::sql::<BigInt>("configuration_revision"),
                    diesel::dsl::sql::<BigInt>("binding_revision"),
                ))
                .load::<(i64, i64)>(connection)?;
            let seat_rows = seats::table
                .order(seats::seat_id)
                .select((seats::seat_id, seats::seat_code))
                .load::<(String, String)>(connection)?;
            let account_rows = accounts::table
                .order(accounts::account_id)
                .select((
                    accounts::account_id,
                    accounts::domjudge_username,
                    accounts::credential_vault_record_id,
                    diesel::dsl::sql::<BigInt>("credential_revision"),
                ))
                .load::<(String, String, String, i64)>(connection)?;
            let mapping_rows = account_mappings::table
                .order(account_mappings::seat_id)
                .select((account_mappings::seat_id, account_mappings::account_id))
                .load::<(String, String)>(connection)?;
            let binding_rows = device_bindings::table
                .order(device_bindings::seat_id)
                .select((
                    device_bindings::seat_id,
                    device_bindings::device_pk,
                    diesel::dsl::sql::<BigInt>("binding_revision"),
                ))
                .load::<(String, String, i64)>(connection)?;
            let vault_rows = server_vault_records::table
                .order(server_vault_records::vault_record_id)
                .select((
                    server_vault_records::vault_record_id,
                    server_vault_records::record_type,
                    server_vault_records::subject_id,
                    server_vault_records::nonce,
                    server_vault_records::ciphertext,
                ))
                .load::<VaultSnapshotRow>(connection)?;
            let candidate_rows = pending_import_candidate::table
                .order(pending_import_candidate::singleton)
                .select((
                    pending_import_candidate::candidate_id,
                    pending_import_candidate::payload_vault_record_id,
                    pending_import_candidate::preview_token_hash,
                ))
                .load::<(String, String, Vec<u8>)>(connection)?;
            let audit_rows = audit_events::table
                .order(audit_events::audit_event_id)
                .select((audit_events::audit_event_id, audit_events::action_kind))
                .load::<(String, String)>(connection)?;
            let runtime_counts = diesel::sql_query(
                "SELECT \
                 (SELECT COUNT(*) FROM commands) AS command_count, \
                 (SELECT COUNT(*) FROM observed_device_states) AS observed_device_state_count",
            )
            .get_result::<RuntimeTableCounts>(connection)?;
            Ok::<FullRollbackSnapshot, diesel::result::Error>(FullRollbackSnapshot {
                revisions,
                seat_rows,
                account_rows,
                mapping_rows,
                binding_rows,
                vault_rows,
                candidate_rows,
                audit_rows,
                command_count: runtime_counts.command_count,
                observed_device_state_count: runtime_counts.observed_device_state_count,
            })
        })
        .await
        .map_err(|_| TestFailure::EvidenceFailed)?
        .map_err(|_| TestFailure::EvidenceFailed)
}

async fn seed_expired_candidate(
    database: &Database,
    candidate_id: Uuid,
    payload_id: Uuid,
    audit_event_id: Uuid,
) -> Result<(), TestFailure> {
    let candidate_id = candidate_id.to_string();
    let payload_id = payload_id.to_string();
    let audit_event_id = audit_event_id.to_string();
    let correlation_id = Uuid::now_v7().to_string();
    database
        .interact(move |connection| {
            diesel::sql_query(
                "INSERT INTO server_vault_records \
                 (vault_record_id, record_type, subject_id, nonce, ciphertext) \
                 VALUES (?, 'import_payload', ?, x'01', x'02')",
            )
            .bind::<Text, _>(&payload_id)
            .bind::<Text, _>(&candidate_id)
            .execute(connection)?;
            diesel::sql_query(
                "INSERT INTO pending_import_candidate \
                 (singleton, candidate_id, expires_at, baseline_configuration_revision, \
                  baseline_binding_revision, preview_token_hash, payload_vault_record_id, \
                  redacted_preview_json) \
                 VALUES (1, ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 second'), \
                         0, 0, zeroblob(32), ?, '{}')",
            )
            .bind::<Text, _>(&candidate_id)
            .bind::<Text, _>(&payload_id)
            .execute(connection)?;
            diesel::sql_query(
                "INSERT INTO audit_events \
                 (audit_event_id, occurred_at, actor, action_kind, resource_type, resource_id, \
                  result, reason_code, correlation_id, group_correlation_id, \
                  redacted_detail_json) \
                 VALUES (?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'system:test', \
                         'fixture', 'import_candidate', ?, 'succeeded', NULL, ?, NULL, '{}')",
            )
            .bind::<Text, _>(audit_event_id)
            .bind::<Text, _>(candidate_id)
            .bind::<Text, _>(correlation_id)
            .execute(connection)?;
            Ok::<(), diesel::result::Error>(())
        })
        .await
        .map_err(|_| TestFailure::FixtureFailed)?
        .map_err(|_| TestFailure::FixtureFailed)
}

async fn rollback_snapshot(database: &Database) -> Result<RollbackSnapshot, TestFailure> {
    database
        .interact(|connection| {
            diesel::sql_query(
                "SELECT candidate_id, payload_vault_record_id, \
                 (SELECT COUNT(*) FROM pending_import_candidate) AS candidate_count, \
                 (SELECT COUNT(*) FROM server_vault_records v \
                   WHERE v.vault_record_id = pending_import_candidate.payload_vault_record_id) \
                   AS old_payload_count, \
                 (SELECT COUNT(*) FROM audit_events \
                   WHERE action_kind = 'expire_import_candidate') AS expiry_audit_count, \
                 (SELECT COUNT(*) FROM audit_events) AS audit_count \
                 FROM pending_import_candidate WHERE singleton = 1",
            )
            .get_result::<RollbackSnapshot>(connection)
        })
        .await
        .map_err(|_| TestFailure::EvidenceFailed)?
        .map_err(|_| TestFailure::EvidenceFailed)
}

fn data_version(connection: &mut SqliteConnection) -> Result<i64, TestFailure> {
    diesel::dsl::sql::<BigInt>("PRAGMA data_version")
        .get_result(connection)
        .map_err(|_| TestFailure::EvidenceFailed)
}

struct TestDatabase {
    database: Database,
    path: PathBuf,
}

impl TestDatabase {
    async fn new() -> Result<Self, TestFailure> {
        let path = std::env::temp_dir().join(format!(
            "natsume-import-rollback-test-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .map_err(|_| TestFailure::FixtureFailed)?;
        Ok(Self { database, path })
    }

    fn observer(&self) -> Result<SqliteConnection, TestFailure> {
        let path = self.path.to_str().ok_or(TestFailure::FixtureFailed)?;
        SqliteConnection::establish(path).map_err(|_| TestFailure::FixtureFailed)
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _database_result = fs::remove_file(&self.path);
        let _wal_result = fs::remove_file(format!("{}-wal", self.path.display()));
        let _shm_result = fs::remove_file(format!("{}-shm", self.path.display()));
    }
}

#[derive(Debug, PartialEq, Eq, QueryableByName)]
struct RollbackSnapshot {
    #[diesel(sql_type = Text)]
    candidate_id: String,
    #[diesel(sql_type = Text)]
    payload_vault_record_id: String,
    #[diesel(sql_type = BigInt)]
    candidate_count: i64,
    #[diesel(sql_type = BigInt)]
    old_payload_count: i64,
    #[diesel(sql_type = BigInt)]
    expiry_audit_count: i64,
    #[diesel(sql_type = BigInt)]
    audit_count: i64,
}

type VaultSnapshotRow = (String, String, String, Vec<u8>, Vec<u8>);

#[derive(QueryableByName)]
struct RuntimeTableCounts {
    #[diesel(sql_type = BigInt)]
    command_count: i64,
    #[diesel(sql_type = BigInt)]
    observed_device_state_count: i64,
}

#[derive(Debug, PartialEq, Eq)]
struct FullRollbackSnapshot {
    revisions: Vec<(i64, i64)>,
    seat_rows: Vec<(String, String)>,
    account_rows: Vec<(String, String, String, i64)>,
    mapping_rows: Vec<(String, String)>,
    binding_rows: Vec<(String, String, i64)>,
    vault_rows: Vec<VaultSnapshotRow>,
    candidate_rows: Vec<(String, String, Vec<u8>)>,
    audit_rows: Vec<(String, String)>,
    command_count: i64,
    observed_device_state_count: i64,
}

#[derive(Debug, Snafu)]
enum TestFailure {
    #[snafu(display("the import rollback fixture failed"))]
    FixtureFailed,
    #[snafu(display("the import rollback evidence could not be read"))]
    EvidenceFailed,
    #[snafu(display("the duplicate create audit failure was expected"))]
    ExpectedCreateAuditFailure,
    #[snafu(display("the duplicate commit audit failure was expected"))]
    ExpectedCommitAuditFailure,
    #[snafu(display("the import store failure classification changed"))]
    UnexpectedStoreFailure,
    #[snafu(display("the compound expiry and create mutation did not roll back"))]
    CompoundMutationDidNotRollBack,
    #[snafu(display("the committed import mutation did not roll back"))]
    CommitMutationDidNotRollBack,
    #[snafu(display("a rejected-audit failure escaped its frozen classification"))]
    RejectionAuditFailureEscaped,
    #[snafu(display("a rejected-audit failure changed persisted state"))]
    RejectedAuditFailureWroteData,
    #[snafu(display("invalid commit-stage facts were accepted"))]
    InvalidCommitFactsWereAccepted,
    #[snafu(display("invalid commit-stage facts escaped persistence classification"))]
    InvalidCommitClassificationChanged,
}
