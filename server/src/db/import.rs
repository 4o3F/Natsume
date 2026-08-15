use std::collections::{BTreeMap, BTreeSet};

use diesel::{
    ExpressionMethods, JoinOnDsl, NullableExpressionMethods, OptionalExtension, QueryDsl,
    QueryableByName, RunQueryDsl,
    sql_types::{BigInt, Integer, Text},
    sqlite::SqliteConnection,
};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    application::import::{
        CandidateRowFacts, IMPORT_CANDIDATE_TTL_SECONDS, ImportBindingImpact, ImportError,
        ImportMappingChange, RedactedImportPreview,
    },
    audit::{self, AuditEvent, AuditEventId, CorrelationId},
    db::{
        Database,
        schema::{
            account_mappings, accounts, device_bindings, pending_import_candidate,
            revision_counters, seats, server_vault_records,
        },
    },
};

const IMPORT_PAYLOAD_RECORD_TYPE: &str = "import_payload";

pub(crate) struct CreatedCandidateFacts {
    pub(crate) candidate_id: Uuid,
    pub(crate) expires_at: String,
    pub(crate) baseline_configuration_revision: i64,
    pub(crate) baseline_binding_revision: i64,
    pub(crate) diff: RedactedImportPreview,
}

pub(crate) async fn create_import_candidate(
    database: &Database,
    candidate_rows: Vec<CandidateRowFacts>,
    preview_token_hash: [u8; 32],
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
    correlation_id: CorrelationId,
) -> Result<CreatedCandidateFacts, ImportError> {
    let request = CandidateCreationRequest {
        candidate_rows,
        preview_token_hash,
        nonce,
        ciphertext,
        correlation_id,
    };
    create_import_candidate_with_ids(
        database,
        request,
        AuditEventId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(ImportError::from)
}

struct CandidateCreationRequest {
    candidate_rows: Vec<CandidateRowFacts>,
    preview_token_hash: [u8; 32],
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
    correlation_id: CorrelationId,
}

async fn create_import_candidate_with_ids(
    database: &Database,
    request: CandidateCreationRequest,
    expiry_audit_event_id: AuditEventId,
    create_audit_event_id: AuditEventId,
) -> Result<CreatedCandidateFacts, ImportStoreError> {
    let input = CandidateCreationInput {
        request,
        candidate_id: Uuid::now_v7(),
        payload_vault_record_id: Uuid::now_v7(),
        expiry_audit_event_id,
        create_audit_event_id,
    };
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                create_import_candidate_in_transaction(connection, &input)
            })
        })
        .await
        .map_err(|_| ImportStoreError::AcquireFailed)?
}

struct CandidateCreationInput {
    request: CandidateCreationRequest,
    candidate_id: Uuid,
    payload_vault_record_id: Uuid,
    expiry_audit_event_id: AuditEventId,
    create_audit_event_id: AuditEventId,
}

fn create_import_candidate_in_transaction(
    connection: &mut SqliteConnection,
    input: &CandidateCreationInput,
) -> Result<CreatedCandidateFacts, ImportStoreError> {
    if let Some(pending) = read_pending_candidate(connection)? {
        match pending.expiry_state {
            0 => return Err(ImportStoreError::CandidatePending),
            1 => expire_pending_candidate(
                connection,
                &pending,
                input.request.correlation_id,
                input.expiry_audit_event_id,
            )?,
            _ => return Err(ImportStoreError::InvalidPersistedFacts),
        }
    }

    let (baseline_configuration_revision, baseline_binding_revision) =
        read_revision_counters(connection)?;
    let current_seats = read_current_seats(connection)?;
    let diff = compute_diff(&current_seats, &input.request.candidate_rows)?;
    let redacted_preview_json =
        serde_json::to_string(&diff).map_err(|_| ImportStoreError::PreviewSerializationFailed)?;

    diesel::insert_into(server_vault_records::table)
        .values((
            server_vault_records::vault_record_id.eq(input.payload_vault_record_id.to_string()),
            server_vault_records::record_type.eq(IMPORT_PAYLOAD_RECORD_TYPE),
            server_vault_records::subject_id.eq(input.candidate_id.to_string()),
            server_vault_records::nonce.eq(input.request.nonce.as_slice()),
            server_vault_records::ciphertext.eq(input.request.ciphertext.as_slice()),
        ))
        .execute(connection)
        .map_err(|_| ImportStoreError::VaultInsertFailed)?;

    let expires_at = import_expiry(connection)?;
    diesel::insert_into(pending_import_candidate::table)
        .values((
            pending_import_candidate::singleton.eq(Some(1_i32)),
            pending_import_candidate::candidate_id.eq(input.candidate_id.to_string()),
            pending_import_candidate::expires_at.eq(&expires_at),
            pending_import_candidate::baseline_configuration_revision
                .eq(diesel::dsl::sql::<Integer>("")
                    .bind::<BigInt, _>(baseline_configuration_revision)),
            pending_import_candidate::baseline_binding_revision
                .eq(diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(baseline_binding_revision)),
            pending_import_candidate::preview_token_hash
                .eq(input.request.preview_token_hash.as_slice()),
            pending_import_candidate::payload_vault_record_id
                .eq(input.payload_vault_record_id.to_string()),
            pending_import_candidate::redacted_preview_json.eq(&redacted_preview_json),
        ))
        .execute(connection)
        .map_err(|_| ImportStoreError::CandidateInsertFailed)?;

    let event = AuditEvent::import_candidate_created(
        input.create_audit_event_id,
        input.request.correlation_id,
        input.candidate_id,
        diff.seats_added().len(),
        diff.seats_removed().len(),
        diff.mappings_changed().len(),
        diff.binding_impacts().len(),
    );
    audit::insert_diesel(connection, &event).map_err(|_| ImportStoreError::AuditInsertFailed)?;

    Ok(CreatedCandidateFacts {
        candidate_id: input.candidate_id,
        expires_at,
        baseline_configuration_revision,
        baseline_binding_revision,
        diff,
    })
}

#[derive(QueryableByName)]
struct PendingCandidateRow {
    #[diesel(sql_type = Text)]
    candidate_id: String,
    #[diesel(sql_type = Text)]
    payload_vault_record_id: String,
    #[diesel(sql_type = BigInt)]
    expiry_state: i64,
}

fn read_pending_candidate(
    connection: &mut SqliteConnection,
) -> Result<Option<PendingCandidateRow>, ImportStoreError> {
    diesel::sql_query(
        "SELECT candidate_id, payload_vault_record_id, \
         CASE \
           WHEN strftime('%Y-%m-%dT%H:%M:%fZ', expires_at) IS NULL \
             OR expires_at <> strftime('%Y-%m-%dT%H:%M:%fZ', expires_at) THEN -1 \
           WHEN expires_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now') THEN 1 \
           ELSE 0 \
         END AS expiry_state \
         FROM pending_import_candidate WHERE singleton = 1",
    )
    .get_result(connection)
    .optional()
    .map_err(|_| ImportStoreError::PendingReadFailed)
}

fn expire_pending_candidate(
    connection: &mut SqliteConnection,
    pending: &PendingCandidateRow,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), ImportStoreError> {
    let candidate_id = canonical_uuid_v7(&pending.candidate_id)?;
    let removed_candidate = diesel::delete(
        pending_import_candidate::table
            .filter(pending_import_candidate::singleton.eq(Some(1_i32)))
            .filter(pending_import_candidate::candidate_id.eq(&pending.candidate_id)),
    )
    .execute(connection)
    .map_err(|_| ImportStoreError::CandidateDeleteFailed)?;
    if removed_candidate != 1 {
        return Err(ImportStoreError::InvalidPersistedFacts);
    }

    let removed_payload = diesel::delete(
        server_vault_records::table
            .filter(server_vault_records::vault_record_id.eq(&pending.payload_vault_record_id))
            .filter(server_vault_records::record_type.eq(IMPORT_PAYLOAD_RECORD_TYPE))
            .filter(server_vault_records::subject_id.eq(&pending.candidate_id)),
    )
    .execute(connection)
    .map_err(|_| ImportStoreError::VaultDeleteFailed)?;
    if removed_payload != 1 {
        return Err(ImportStoreError::InvalidPersistedFacts);
    }

    let event = AuditEvent::import_candidate_expired(audit_event_id, correlation_id, candidate_id);
    audit::insert_diesel(connection, &event).map_err(|_| ImportStoreError::AuditInsertFailed)
}

fn canonical_uuid_v7(value: &str) -> Result<Uuid, ImportStoreError> {
    let parsed = Uuid::parse_str(value).map_err(|_| ImportStoreError::InvalidPersistedFacts)?;
    if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != value {
        return Err(ImportStoreError::InvalidPersistedFacts);
    }
    Ok(parsed)
}

fn read_revision_counters(
    connection: &mut SqliteConnection,
) -> Result<(i64, i64), ImportStoreError> {
    let (configuration_revision, binding_revision) = revision_counters::table
        .filter(revision_counters::singleton.eq(Some(1_i32)))
        .select((
            diesel::dsl::sql::<BigInt>("configuration_revision"),
            diesel::dsl::sql::<BigInt>("binding_revision"),
        ))
        .first::<(i64, i64)>(connection)
        .map_err(|_| ImportStoreError::RevisionsReadFailed)?;
    if configuration_revision < 0 || binding_revision < 0 {
        return Err(ImportStoreError::InvalidPersistedFacts);
    }
    Ok((configuration_revision, binding_revision))
}

struct CurrentSeatFacts {
    current_domjudge_username: Option<String>,
    device_id: Option<String>,
}

fn read_current_seats(
    connection: &mut SqliteConnection,
) -> Result<BTreeMap<String, CurrentSeatFacts>, ImportStoreError> {
    let rows = seats::table
        .left_join(account_mappings::table.on(account_mappings::seat_id.eq(seats::seat_id)))
        .left_join(accounts::table.on(account_mappings::account_id.eq(accounts::account_id)))
        .left_join(device_bindings::table.on(device_bindings::seat_id.eq(seats::seat_id)))
        .select((
            seats::seat_code,
            accounts::domjudge_username.nullable(),
            device_bindings::device_pk.nullable(),
        ))
        .order(seats::seat_code)
        .load::<(String, Option<String>, Option<String>)>(connection)
        .map_err(|_| ImportStoreError::CurrentFactsReadFailed)?;

    let mut current = BTreeMap::new();
    for (seat_code, current_domjudge_username, device_id) in rows {
        if current
            .insert(
                seat_code,
                CurrentSeatFacts {
                    current_domjudge_username,
                    device_id,
                },
            )
            .is_some()
        {
            return Err(ImportStoreError::InvalidPersistedFacts);
        }
    }
    Ok(current)
}

fn compute_diff(
    current: &BTreeMap<String, CurrentSeatFacts>,
    candidate_rows: &[CandidateRowFacts],
) -> Result<RedactedImportPreview, ImportStoreError> {
    let mut candidate = BTreeMap::new();
    let mut candidate_accounts = BTreeSet::new();
    for row in candidate_rows {
        if candidate
            .insert(row.seat_code.as_str(), row.domjudge_username.as_str())
            .is_some()
            || !candidate_accounts.insert(row.domjudge_username.as_str())
        {
            return Err(ImportStoreError::InvalidCandidateFacts);
        }
    }
    if candidate.is_empty() {
        return Err(ImportStoreError::InvalidCandidateFacts);
    }

    let seats_added = candidate
        .keys()
        .filter(|seat_code| !current.contains_key(**seat_code))
        .map(|seat_code| (*seat_code).to_owned())
        .collect();
    let mut seats_removed = Vec::new();
    let mut mappings_changed = Vec::new();
    let mut unchanged_count = 0;
    let mut binding_impacts = Vec::new();

    for (seat_code, facts) in current {
        let Some(candidate_username) = candidate.get(seat_code.as_str()) else {
            seats_removed.push(seat_code.clone());
            if let Some(device_id) = &facts.device_id {
                binding_impacts.push(ImportBindingImpact::new(
                    seat_code.clone(),
                    device_id.clone(),
                ));
            }
            continue;
        };
        if facts.current_domjudge_username.as_deref() == Some(*candidate_username) {
            unchanged_count += 1;
        } else {
            mappings_changed.push(ImportMappingChange::new(
                seat_code.clone(),
                facts.current_domjudge_username.clone(),
                (*candidate_username).to_owned(),
            ));
        }
    }

    Ok(RedactedImportPreview::new(
        seats_added,
        seats_removed,
        mappings_changed,
        unchanged_count,
        candidate_accounts.len(),
        binding_impacts,
    ))
}

#[derive(QueryableByName)]
struct ExpiryRow {
    #[diesel(sql_type = Text)]
    expires_at: String,
}

fn import_expiry(connection: &mut SqliteConnection) -> Result<String, ImportStoreError> {
    let modifier = format!("+{IMPORT_CANDIDATE_TTL_SECONDS} seconds");
    diesel::sql_query("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?) AS expires_at")
        .bind::<Text, _>(modifier)
        .get_result::<ExpiryRow>(connection)
        .map(|row| row.expires_at)
        .map_err(|_| ImportStoreError::ExpiryCalculationFailed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
enum ImportStoreError {
    #[snafu(display("an import candidate is already pending"))]
    CandidatePending,
    #[snafu(display("the database connection could not be acquired"))]
    AcquireFailed,
    #[snafu(display("the import candidate transaction failed"))]
    TransactionFailed,
    #[snafu(display("the pending import candidate could not be read"))]
    PendingReadFailed,
    #[snafu(display("persisted import facts were invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("candidate import facts were invalid"))]
    InvalidCandidateFacts,
    #[snafu(display("the pending import candidate could not be deleted"))]
    CandidateDeleteFailed,
    #[snafu(display("the import payload could not be deleted"))]
    VaultDeleteFailed,
    #[snafu(display("the revision counters could not be read"))]
    RevisionsReadFailed,
    #[snafu(display("the current contest facts could not be read"))]
    CurrentFactsReadFailed,
    #[snafu(display("the redacted import preview could not be serialized"))]
    PreviewSerializationFailed,
    #[snafu(display("the import payload could not be persisted"))]
    VaultInsertFailed,
    #[snafu(display("the import candidate expiry could not be calculated"))]
    ExpiryCalculationFailed,
    #[snafu(display("the import candidate could not be persisted"))]
    CandidateInsertFailed,
    #[snafu(display("the import audit event could not be persisted"))]
    AuditInsertFailed,
}

impl From<diesel::result::Error> for ImportStoreError {
    fn from(_source: diesel::result::Error) -> Self {
        Self::TransactionFailed
    }
}

impl From<ImportStoreError> for ImportError {
    fn from(source: ImportStoreError) -> Self {
        match source {
            ImportStoreError::CandidatePending => Self::CandidatePending,
            ImportStoreError::InvalidCandidateFacts => Self::CandidateInvalid,
            ImportStoreError::AcquireFailed
            | ImportStoreError::TransactionFailed
            | ImportStoreError::PendingReadFailed
            | ImportStoreError::InvalidPersistedFacts
            | ImportStoreError::CandidateDeleteFailed
            | ImportStoreError::VaultDeleteFailed
            | ImportStoreError::RevisionsReadFailed
            | ImportStoreError::CurrentFactsReadFailed
            | ImportStoreError::PreviewSerializationFailed
            | ImportStoreError::VaultInsertFailed
            | ImportStoreError::ExpiryCalculationFailed
            | ImportStoreError::CandidateInsertFailed
            | ImportStoreError::AuditInsertFailed => Self::PersistenceFailure,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use diesel::{
        Connection, QueryableByName, RunQueryDsl,
        sql_types::{BigInt, Text},
        sqlite::SqliteConnection,
    };
    use snafu::Snafu;
    use uuid::Uuid;

    use crate::{
        application::import::CandidateRowFacts,
        audit::{AuditEventId, CorrelationId},
        db::{Database, DatabaseConfig},
    };

    use super::{CandidateCreationRequest, ImportStoreError, create_import_candidate_with_ids};

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

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the import rollback fixture failed"))]
        FixtureFailed,
        #[snafu(display("the import rollback evidence could not be read"))]
        EvidenceFailed,
        #[snafu(display("the duplicate create audit failure was expected"))]
        ExpectedCreateAuditFailure,
        #[snafu(display("the import store failure classification changed"))]
        UnexpectedStoreFailure,
        #[snafu(display("the compound expiry and create mutation did not roll back"))]
        CompoundMutationDidNotRollBack,
    }
}
