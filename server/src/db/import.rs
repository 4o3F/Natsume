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
        CandidateRowFacts, CommitCandidatePayload, CommittedImportFacts,
        IMPORT_CANDIDATE_TTL_SECONDS, ImportBindingImpact, ImportError, ImportMappingChange,
        MAX_IMPORT_ROWS, RedactedImportPreview, SealedCommitRow,
    },
    audit::{
        self, AuditEvent, AuditEventId, CorrelationId, ImportCommitAuditFacts,
        ImportCommitRejectionReason,
    },
    db::{
        Database,
        schema::{
            account_mappings, accounts, device_bindings, pending_import_candidate,
            revision_counters, seats, server_vault_records,
        },
    },
    vault::VaultRecordType,
};

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

pub(crate) async fn audit_invalid_import_upload(
    database: &Database,
    correlation_id: CorrelationId,
) -> Result<(), ImportError> {
    audit_invalid_import_upload_with_id(
        database,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(ImportError::from)
}

async fn audit_invalid_import_upload_with_id(
    database: &Database,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), ImportStoreError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                let event = AuditEvent::import_candidate_rejected(audit_event_id, correlation_id);
                audit::insert_diesel(connection, &event)
                    .map_err(|_| ImportStoreError::AuditInsertFailed)
            })
        })
        .await
        .map_err(|_| ImportStoreError::AcquireFailed)?
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
            1 => {
                let candidate_id = canonical_uuid_v7(&pending.candidate_id)?;
                expire_pending_candidate_tolerant(
                    connection,
                    &pending,
                    candidate_id,
                    input.request.correlation_id,
                    input.expiry_audit_event_id,
                )?;
            }
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
            server_vault_records::record_type.eq(VaultRecordType::ImportPayload.as_str()),
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

pub(crate) async fn read_commit_candidate(
    database: &Database,
    candidate_id: Uuid,
    correlation_id: CorrelationId,
) -> Result<CommitCandidatePayload, ImportError> {
    let outcome = read_commit_candidate_with_ids(
        database,
        candidate_id,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(ImportError::from)?;
    match outcome {
        CandidateReadOutcome::Available(payload) => Ok(payload),
        CandidateReadOutcome::Unavailable => Err(ImportError::CandidateUnavailable),
    }
}

enum CandidateReadOutcome {
    Available(CommitCandidatePayload),
    Unavailable,
}

async fn read_commit_candidate_with_ids(
    database: &Database,
    candidate_id: Uuid,
    correlation_id: CorrelationId,
    expiry_audit_event_id: AuditEventId,
) -> Result<CandidateReadOutcome, ImportStoreError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                let Some(pending) = read_pending_candidate(connection)? else {
                    return Ok(CandidateReadOutcome::Unavailable);
                };
                if pending.candidate_id != candidate_id.to_string() {
                    return Ok(CandidateReadOutcome::Unavailable);
                }
                match pending.expiry_state {
                    1 => {
                        expire_pending_candidate_tolerant(
                            connection,
                            &pending,
                            candidate_id,
                            correlation_id,
                            expiry_audit_event_id,
                        )?;
                        return Ok(CandidateReadOutcome::Unavailable);
                    }
                    0 => {}
                    _ => return Err(ImportStoreError::InvalidPersistedFacts),
                }

                let preview_token_hash = pending_preview_token_hash(&pending)?;
                let payload_vault_record_id = canonical_uuid_v7(&pending.payload_vault_record_id)?;
                let payload = server_vault_records::table
                    .filter(
                        server_vault_records::vault_record_id.eq(&pending.payload_vault_record_id),
                    )
                    .filter(
                        server_vault_records::record_type
                            .eq(VaultRecordType::ImportPayload.as_str()),
                    )
                    .filter(server_vault_records::subject_id.eq(&pending.candidate_id))
                    .select((
                        server_vault_records::nonce,
                        server_vault_records::ciphertext,
                    ))
                    .first::<(Vec<u8>, Vec<u8>)>(connection)
                    .optional()
                    .map_err(|_| ImportStoreError::VaultReadFailed)?
                    .ok_or(ImportStoreError::InvalidPersistedFacts)?;

                Ok(CandidateReadOutcome::Available(CommitCandidatePayload {
                    candidate_id,
                    preview_token_hash,
                    payload_vault_record_id,
                    nonce: payload.0,
                    ciphertext: payload.1,
                }))
            })
        })
        .await
        .map_err(|_| ImportStoreError::AcquireFailed)?
}

pub(crate) async fn audit_preview_token_mismatch(
    database: &Database,
    candidate_id: Uuid,
    expected_preview_token_hash: [u8; 32],
    correlation_id: CorrelationId,
) -> Result<(), ImportError> {
    audit_preview_token_mismatch_with_ids(
        database,
        candidate_id,
        expected_preview_token_hash,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(ImportError::from)
}

async fn audit_preview_token_mismatch_with_ids(
    database: &Database,
    candidate_id: Uuid,
    expected_preview_token_hash: [u8; 32],
    correlation_id: CorrelationId,
    expiry_audit_event_id: AuditEventId,
    rejection_audit_event_id: AuditEventId,
) -> Result<(), ImportStoreError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                let Some(pending) = read_pending_candidate(connection)? else {
                    return Ok(());
                };
                if pending.candidate_id != candidate_id.to_string() {
                    return Ok(());
                }
                match pending.expiry_state {
                    1 => {
                        return expire_pending_candidate_tolerant(
                            connection,
                            &pending,
                            candidate_id,
                            correlation_id,
                            expiry_audit_event_id,
                        );
                    }
                    0 => {}
                    _ => return Err(ImportStoreError::InvalidPersistedFacts),
                }
                if pending_preview_token_hash(&pending)? != expected_preview_token_hash {
                    return Ok(());
                }

                let event = AuditEvent::import_commit_rejected(
                    rejection_audit_event_id,
                    correlation_id,
                    candidate_id,
                    ImportCommitRejectionReason::PreviewTokenMismatch,
                );
                // §3.4 forbids audit failure from turning token mismatch into an existence oracle.
                if audit::insert_diesel(connection, &event).is_err() {
                    tracing::warn!(
                        discriminant = "preview_token_mismatch_audit_write_failed",
                        "import rejection audit write failed"
                    );
                }
                Ok(())
            })
        })
        .await
        .map_err(|_| ImportStoreError::AcquireFailed)?
}

pub(crate) async fn commit_import(
    database: &Database,
    candidate_id: Uuid,
    expected_preview_token_hash: [u8; 32],
    expected_payload_vault_record_id: Uuid,
    sealed_rows: Vec<SealedCommitRow>,
    correlation_id: CorrelationId,
) -> Result<CommittedImportFacts, ImportError> {
    let request = CommitRequest {
        candidate_id,
        expected_preview_token_hash,
        expected_payload_vault_record_id,
        sealed_rows,
        correlation_id,
    };
    let outcome = commit_import_with_ids(
        database,
        request,
        AuditEventId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(ImportError::from)?;
    match outcome {
        CommitOutcome::Committed(facts) => Ok(facts),
        CommitOutcome::Unavailable => Err(ImportError::CandidateUnavailable),
        CommitOutcome::Stale => Err(ImportError::PreviewStale),
    }
}

struct CommitRequest {
    candidate_id: Uuid,
    expected_preview_token_hash: [u8; 32],
    expected_payload_vault_record_id: Uuid,
    sealed_rows: Vec<SealedCommitRow>,
    correlation_id: CorrelationId,
}

enum CommitOutcome {
    Committed(CommittedImportFacts),
    Unavailable,
    Stale,
}

async fn commit_import_with_ids(
    database: &Database,
    request: CommitRequest,
    expiry_audit_event_id: AuditEventId,
    commit_audit_event_id: AuditEventId,
) -> Result<CommitOutcome, ImportStoreError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                let Some(pending) = read_pending_candidate(connection)? else {
                    return Ok(CommitOutcome::Unavailable);
                };
                if pending.candidate_id != request.candidate_id.to_string() {
                    return Ok(CommitOutcome::Unavailable);
                }
                match pending.expiry_state {
                    1 => {
                        expire_pending_candidate_tolerant(
                            connection,
                            &pending,
                            request.candidate_id,
                            request.correlation_id,
                            expiry_audit_event_id,
                        )?;
                        return Ok(CommitOutcome::Unavailable);
                    }
                    0 => {}
                    _ => return Err(ImportStoreError::InvalidPersistedFacts),
                }
                if pending_preview_token_hash(&pending)? != request.expected_preview_token_hash
                    || pending.payload_vault_record_id
                        != request.expected_payload_vault_record_id.to_string()
                {
                    return Ok(CommitOutcome::Unavailable);
                }

                let (configuration_revision, binding_revision) =
                    read_revision_counters(connection)?;
                if configuration_revision != pending.baseline_configuration_revision
                    || binding_revision != pending.baseline_binding_revision
                {
                    let event = AuditEvent::import_commit_rejected(
                        commit_audit_event_id,
                        request.correlation_id,
                        request.candidate_id,
                        ImportCommitRejectionReason::BaselineStale,
                    );
                    // §3.4 keeps stale classification stable under rejected-audit failure.
                    if audit::insert_diesel(connection, &event).is_err() {
                        tracing::warn!(
                            discriminant = "baseline_stale_audit_write_failed",
                            "import rejection audit write failed"
                        );
                    }
                    return Ok(CommitOutcome::Stale);
                }

                let mutation = apply_committed_candidate(
                    connection,
                    &request.sealed_rows,
                    pending.baseline_configuration_revision,
                    pending.baseline_binding_revision,
                )?;
                delete_committed_candidate(connection, &pending, request.candidate_id)?;
                let event = AuditEvent::import_committed(
                    commit_audit_event_id,
                    request.correlation_id,
                    request.candidate_id,
                    &ImportCommitAuditFacts {
                        seats_added_count: mutation.seats_added_count,
                        seats_removed_count: mutation.seats_removed_count,
                        mappings_changed_count: mutation.mappings_changed_count,
                        binding_impact_count: mutation.binding_impact_count,
                        credential_revision_advanced_count: mutation
                            .credential_revision_advanced_count,
                        configuration_revision_advanced: mutation.configuration_revision_advanced,
                        binding_revision_advanced: mutation.binding_revision_advanced,
                    },
                );
                audit::insert_diesel(connection, &event)
                    .map_err(|_| ImportStoreError::AuditInsertFailed)?;
                Ok(CommitOutcome::Committed(CommittedImportFacts::new(
                    mutation.configuration_revision,
                    mutation.binding_revision,
                )))
            })
        })
        .await
        .map_err(|_| ImportStoreError::AcquireFailed)?
}

pub(crate) async fn discard_import(
    database: &Database,
    candidate_id: Uuid,
    correlation_id: CorrelationId,
) -> Result<(), ImportError> {
    let outcome = discard_import_with_ids(
        database,
        candidate_id,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(ImportError::from)?;
    match outcome {
        DiscardOutcome::Discarded => Ok(()),
        DiscardOutcome::Unavailable => Err(ImportError::CandidateUnavailable),
    }
}

enum DiscardOutcome {
    Discarded,
    Unavailable,
}

async fn discard_import_with_ids(
    database: &Database,
    candidate_id: Uuid,
    correlation_id: CorrelationId,
    expiry_audit_event_id: AuditEventId,
    discard_audit_event_id: AuditEventId,
) -> Result<DiscardOutcome, ImportStoreError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                let Some(pending) = read_pending_candidate(connection)? else {
                    return Ok(DiscardOutcome::Unavailable);
                };
                if pending.candidate_id != candidate_id.to_string() {
                    return Ok(DiscardOutcome::Unavailable);
                }
                match pending.expiry_state {
                    1 => {
                        expire_pending_candidate_tolerant(
                            connection,
                            &pending,
                            candidate_id,
                            correlation_id,
                            expiry_audit_event_id,
                        )?;
                        return Ok(DiscardOutcome::Unavailable);
                    }
                    -1 | 0 => {}
                    _ => return Err(ImportStoreError::InvalidPersistedFacts),
                }

                delete_candidate_and_optional_payload(connection, &pending)?;
                let event = AuditEvent::import_candidate_discarded(
                    discard_audit_event_id,
                    correlation_id,
                    candidate_id,
                );
                audit::insert_diesel(connection, &event)
                    .map_err(|_| ImportStoreError::AuditInsertFailed)?;
                Ok(DiscardOutcome::Discarded)
            })
        })
        .await
        .map_err(|_| ImportStoreError::AcquireFailed)?
}

#[derive(QueryableByName)]
struct PendingCandidateRow {
    #[diesel(sql_type = Text)]
    candidate_id: String,
    #[diesel(sql_type = Text)]
    payload_vault_record_id: String,
    #[diesel(sql_type = BigInt)]
    baseline_configuration_revision: i64,
    #[diesel(sql_type = BigInt)]
    baseline_binding_revision: i64,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    preview_token_hash: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    expiry_state: i64,
}

fn read_pending_candidate(
    connection: &mut SqliteConnection,
) -> Result<Option<PendingCandidateRow>, ImportStoreError> {
    diesel::sql_query(
        "SELECT candidate_id, payload_vault_record_id, \
         baseline_configuration_revision, baseline_binding_revision, preview_token_hash, \
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

fn pending_preview_token_hash(pending: &PendingCandidateRow) -> Result<[u8; 32], ImportStoreError> {
    pending
        .preview_token_hash
        .as_slice()
        .try_into()
        .map_err(|_| ImportStoreError::InvalidPersistedFacts)
}

fn expire_pending_candidate_tolerant(
    connection: &mut SqliteConnection,
    pending: &PendingCandidateRow,
    candidate_id: Uuid,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), ImportStoreError> {
    delete_candidate_and_optional_payload(connection, pending)?;
    let event = AuditEvent::import_candidate_expired(audit_event_id, correlation_id, candidate_id);
    audit::insert_diesel(connection, &event).map_err(|_| ImportStoreError::AuditInsertFailed)
}

fn delete_candidate_and_optional_payload(
    connection: &mut SqliteConnection,
    pending: &PendingCandidateRow,
) -> Result<(), ImportStoreError> {
    let removed_candidate = diesel::delete(
        pending_import_candidate::table
            .filter(pending_import_candidate::singleton.eq(Some(1_i32)))
            .filter(pending_import_candidate::candidate_id.eq(&pending.candidate_id)),
    )
    .execute(connection)
    .map_err(|_| ImportStoreError::CandidateDeleteFailed)?;
    if removed_candidate != 1 {
        return Err(ImportStoreError::MutationConflict);
    }
    let removed_payload = diesel::delete(
        server_vault_records::table
            .filter(server_vault_records::vault_record_id.eq(&pending.payload_vault_record_id))
            .filter(server_vault_records::record_type.eq(VaultRecordType::ImportPayload.as_str()))
            .filter(server_vault_records::subject_id.eq(&pending.candidate_id)),
    )
    .execute(connection)
    .map_err(|_| ImportStoreError::VaultDeleteFailed)?;
    if removed_payload > 1 {
        return Err(ImportStoreError::MutationConflict);
    }
    Ok(())
}

fn delete_committed_candidate(
    connection: &mut SqliteConnection,
    pending: &PendingCandidateRow,
    candidate_id: Uuid,
) -> Result<(), ImportStoreError> {
    let removed_candidate = diesel::delete(
        pending_import_candidate::table
            .filter(pending_import_candidate::singleton.eq(Some(1_i32)))
            .filter(pending_import_candidate::candidate_id.eq(candidate_id.to_string())),
    )
    .execute(connection)
    .map_err(|_| ImportStoreError::CandidateDeleteFailed)?;
    let removed_payload = diesel::delete(
        server_vault_records::table
            .filter(server_vault_records::vault_record_id.eq(&pending.payload_vault_record_id))
            .filter(server_vault_records::record_type.eq(VaultRecordType::ImportPayload.as_str()))
            .filter(server_vault_records::subject_id.eq(candidate_id.to_string())),
    )
    .execute(connection)
    .map_err(|_| ImportStoreError::VaultDeleteFailed)?;
    if removed_candidate != 1 || removed_payload != 1 {
        return Err(ImportStoreError::MutationConflict);
    }
    Ok(())
}

struct CurrentAccountFacts {
    account_id: String,
    credential_vault_record_id: String,
    credential_revision: i64,
}

fn read_current_accounts(
    connection: &mut SqliteConnection,
) -> Result<BTreeMap<String, CurrentAccountFacts>, ImportStoreError> {
    let rows = accounts::table
        .left_join(
            server_vault_records::table
                .on(accounts::credential_vault_record_id.eq(server_vault_records::vault_record_id)),
        )
        .select((
            accounts::account_id,
            accounts::domjudge_username,
            accounts::credential_vault_record_id,
            diesel::dsl::sql::<BigInt>("credential_revision"),
            server_vault_records::record_type.nullable(),
            server_vault_records::subject_id.nullable(),
        ))
        .order(accounts::domjudge_username)
        .load::<(String, String, String, i64, Option<String>, Option<String>)>(connection)
        .map_err(|_| ImportStoreError::CurrentFactsReadFailed)?;

    let mut accounts_by_username = BTreeMap::new();
    for (
        account_id,
        domjudge_username,
        credential_vault_record_id,
        credential_revision,
        record_type,
        subject_id,
    ) in rows
    {
        if credential_revision < 1
            || record_type.as_deref() != Some(VaultRecordType::AccountCredential.as_str())
            || subject_id.as_deref() != Some(account_id.as_str())
            || accounts_by_username
                .insert(
                    domjudge_username,
                    CurrentAccountFacts {
                        account_id,
                        credential_vault_record_id,
                        credential_revision,
                    },
                )
                .is_some()
        {
            return Err(ImportStoreError::InvalidPersistedFacts);
        }
    }
    Ok(accounts_by_username)
}

struct CommitMutationFacts {
    configuration_revision: i64,
    binding_revision: i64,
    seats_added_count: usize,
    seats_removed_count: usize,
    mappings_changed_count: usize,
    binding_impact_count: usize,
    credential_revision_advanced_count: usize,
    configuration_revision_advanced: bool,
    binding_revision_advanced: bool,
}

fn apply_committed_candidate(
    connection: &mut SqliteConnection,
    sealed_rows: &[SealedCommitRow],
    baseline_configuration_revision: i64,
    baseline_binding_revision: i64,
) -> Result<CommitMutationFacts, ImportStoreError> {
    let plan = prepare_commit_plan(
        connection,
        sealed_rows,
        baseline_configuration_revision,
        baseline_binding_revision,
    )?;
    replace_seats_and_clear_mappings(connection, &plan)?;
    let final_account_ids = replace_accounts(connection, sealed_rows, &plan)?;
    insert_final_mappings(
        connection,
        sealed_rows,
        &plan.current_seats,
        &final_account_ids,
    )?;
    persist_revisions(connection, &plan)?;
    Ok(CommitMutationFacts {
        configuration_revision: plan.next_configuration_revision,
        binding_revision: plan.next_binding_revision,
        seats_added_count: plan.diff.seats_added().len(),
        seats_removed_count: plan.diff.seats_removed().len(),
        mappings_changed_count: plan.diff.mappings_changed().len(),
        binding_impact_count: plan.diff.binding_impacts().len(),
        credential_revision_advanced_count: sealed_rows.len(),
        configuration_revision_advanced: plan.configuration_revision_advanced,
        binding_revision_advanced: plan.binding_revision_advanced,
    })
}

struct CommitPlan {
    current_seats: BTreeMap<String, CurrentSeatFacts>,
    current_accounts: BTreeMap<String, CurrentAccountFacts>,
    candidate_usernames: BTreeSet<String>,
    diff: RedactedImportPreview,
    baseline_configuration_revision: i64,
    baseline_binding_revision: i64,
    next_configuration_revision: i64,
    next_binding_revision: i64,
    configuration_revision_advanced: bool,
    binding_revision_advanced: bool,
}

fn prepare_commit_plan(
    connection: &mut SqliteConnection,
    sealed_rows: &[SealedCommitRow],
    baseline_configuration_revision: i64,
    baseline_binding_revision: i64,
) -> Result<CommitPlan, ImportStoreError> {
    if sealed_rows.is_empty() || sealed_rows.len() > MAX_IMPORT_ROWS {
        return Err(ImportStoreError::InvalidPersistedFacts);
    }
    let mut candidate_rows = Vec::with_capacity(sealed_rows.len());
    let mut candidate_usernames = BTreeSet::new();
    for row in sealed_rows {
        if row.seat_code.is_empty()
            || row.domjudge_username.is_empty()
            || row.ciphertext.is_empty()
            || !candidate_usernames.insert(row.domjudge_username.clone())
        {
            return Err(ImportStoreError::InvalidPersistedFacts);
        }
        candidate_rows.push(CandidateRowFacts {
            seat_code: row.seat_code.clone(),
            domjudge_username: row.domjudge_username.clone(),
        });
    }
    let current_seats = read_current_seats(connection)?;
    let current_accounts = read_current_accounts(connection)?;
    let diff = compute_diff(&current_seats, &candidate_rows)
        .map_err(|_| ImportStoreError::InvalidPersistedFacts)?;
    let configuration_revision_advanced = !diff.seats_added().is_empty()
        || !diff.seats_removed().is_empty()
        || !diff.mappings_changed().is_empty();
    let binding_revision_advanced = !diff.binding_impacts().is_empty();
    let next_configuration_revision = if configuration_revision_advanced {
        baseline_configuration_revision
            .checked_add(1)
            .ok_or(ImportStoreError::RevisionOverflow)?
    } else {
        baseline_configuration_revision
    };
    let next_binding_revision = if binding_revision_advanced {
        baseline_binding_revision
            .checked_add(1)
            .ok_or(ImportStoreError::RevisionOverflow)?
    } else {
        baseline_binding_revision
    };
    Ok(CommitPlan {
        current_seats,
        current_accounts,
        candidate_usernames,
        diff,
        baseline_configuration_revision,
        baseline_binding_revision,
        next_configuration_revision,
        next_binding_revision,
        configuration_revision_advanced,
        binding_revision_advanced,
    })
}

fn replace_seats_and_clear_mappings(
    connection: &mut SqliteConnection,
    plan: &CommitPlan,
) -> Result<(), ImportStoreError> {
    let removed_mapping_count = diesel::delete(account_mappings::table)
        .execute(connection)
        .map_err(|_| ImportStoreError::MutationFailed)?;
    let current_mapping_count = plan
        .current_seats
        .values()
        .filter(|facts| facts.current_domjudge_username.is_some())
        .count();
    if removed_mapping_count != current_mapping_count {
        return Err(ImportStoreError::MutationConflict);
    }
    for seat_code in plan.diff.seats_removed() {
        let facts = plan
            .current_seats
            .get(seat_code)
            .ok_or(ImportStoreError::InvalidPersistedFacts)?;
        let removed_binding = diesel::delete(
            device_bindings::table.filter(device_bindings::seat_id.eq(&facts.seat_id)),
        )
        .execute(connection)
        .map_err(|_| ImportStoreError::MutationFailed)?;
        if removed_binding != usize::from(facts.device_id.is_some()) {
            return Err(ImportStoreError::MutationConflict);
        }
        let removed_seat = diesel::delete(seats::table.filter(seats::seat_id.eq(&facts.seat_id)))
            .execute(connection)
            .map_err(|_| ImportStoreError::MutationFailed)?;
        if removed_seat != 1 {
            return Err(ImportStoreError::MutationConflict);
        }
    }
    for seat_code in plan.diff.seats_added() {
        let inserted = diesel::insert_into(seats::table)
            .values((seats::seat_id.eq(seat_code), seats::seat_code.eq(seat_code)))
            .execute(connection)
            .map_err(|_| ImportStoreError::MutationFailed)?;
        if inserted != 1 {
            return Err(ImportStoreError::MutationConflict);
        }
    }
    Ok(())
}

fn replace_accounts(
    connection: &mut SqliteConnection,
    sealed_rows: &[SealedCommitRow],
    plan: &CommitPlan,
) -> Result<BTreeMap<String, String>, ImportStoreError> {
    remove_absent_accounts(
        connection,
        &plan.current_accounts,
        &plan.candidate_usernames,
    )?;
    let mut final_account_ids = BTreeMap::new();
    for row in sealed_rows {
        let account_id = if let Some(current) = plan.current_accounts.get(&row.domjudge_username) {
            rotate_account_credential(connection, current, row)?
        } else {
            create_account_credential(connection, row)?
        };
        final_account_ids.insert(row.domjudge_username.clone(), account_id);
    }
    Ok(final_account_ids)
}

fn remove_absent_accounts(
    connection: &mut SqliteConnection,
    current_accounts: &BTreeMap<String, CurrentAccountFacts>,
    candidate_usernames: &BTreeSet<String>,
) -> Result<(), ImportStoreError> {
    for (username, facts) in current_accounts {
        if candidate_usernames.contains(username) {
            continue;
        }
        let removed_account =
            diesel::delete(accounts::table.filter(accounts::account_id.eq(&facts.account_id)))
                .execute(connection)
                .map_err(|_| ImportStoreError::MutationFailed)?;
        let removed_vault = diesel::delete(
            server_vault_records::table
                .filter(server_vault_records::vault_record_id.eq(&facts.credential_vault_record_id))
                .filter(
                    server_vault_records::record_type
                        .eq(VaultRecordType::AccountCredential.as_str()),
                )
                .filter(server_vault_records::subject_id.eq(&facts.account_id)),
        )
        .execute(connection)
        .map_err(|_| ImportStoreError::MutationFailed)?;
        if removed_account != 1 || removed_vault != 1 {
            return Err(ImportStoreError::MutationConflict);
        }
    }
    Ok(())
}

fn rotate_account_credential(
    connection: &mut SqliteConnection,
    current: &CurrentAccountFacts,
    row: &SealedCommitRow,
) -> Result<String, ImportStoreError> {
    let next_credential_revision = current
        .credential_revision
        .checked_add(1)
        .ok_or(ImportStoreError::CredentialRevisionOverflow)?;
    let updated_vault = diesel::update(
        server_vault_records::table
            .filter(server_vault_records::vault_record_id.eq(&current.credential_vault_record_id))
            .filter(
                server_vault_records::record_type.eq(VaultRecordType::AccountCredential.as_str()),
            )
            .filter(server_vault_records::subject_id.eq(&current.account_id)),
    )
    .set((
        server_vault_records::nonce.eq(row.nonce.as_slice()),
        server_vault_records::ciphertext.eq(row.ciphertext.as_slice()),
    ))
    .execute(connection)
    .map_err(|_| ImportStoreError::MutationFailed)?;
    let updated_account = diesel::update(
        accounts::table.filter(accounts::account_id.eq(&current.account_id)),
    )
    .set(
        accounts::credential_revision
            .eq(diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(next_credential_revision)),
    )
    .execute(connection)
    .map_err(|_| ImportStoreError::MutationFailed)?;
    if updated_vault != 1 || updated_account != 1 {
        return Err(ImportStoreError::MutationConflict);
    }
    Ok(current.account_id.clone())
}

fn create_account_credential(
    connection: &mut SqliteConnection,
    row: &SealedCommitRow,
) -> Result<String, ImportStoreError> {
    let account_id = Uuid::now_v7();
    let vault_record_id = Uuid::now_v7();
    let inserted_vault = diesel::insert_into(server_vault_records::table)
        .values((
            server_vault_records::vault_record_id.eq(vault_record_id.to_string()),
            server_vault_records::record_type.eq(VaultRecordType::AccountCredential.as_str()),
            server_vault_records::subject_id.eq(account_id.to_string()),
            server_vault_records::nonce.eq(row.nonce.as_slice()),
            server_vault_records::ciphertext.eq(row.ciphertext.as_slice()),
        ))
        .execute(connection)
        .map_err(|_| ImportStoreError::MutationFailed)?;
    let inserted_account = diesel::insert_into(accounts::table)
        .values((
            accounts::account_id.eq(account_id.to_string()),
            accounts::domjudge_username.eq(&row.domjudge_username),
            accounts::credential_vault_record_id.eq(vault_record_id.to_string()),
            accounts::credential_revision
                .eq(diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(1_i64)),
        ))
        .execute(connection)
        .map_err(|_| ImportStoreError::MutationFailed)?;
    if inserted_vault != 1 || inserted_account != 1 {
        return Err(ImportStoreError::MutationConflict);
    }
    Ok(account_id.to_string())
}

fn insert_final_mappings(
    connection: &mut SqliteConnection,
    sealed_rows: &[SealedCommitRow],
    current_seats: &BTreeMap<String, CurrentSeatFacts>,
    final_account_ids: &BTreeMap<String, String>,
) -> Result<(), ImportStoreError> {
    for row in sealed_rows {
        let seat_id = current_seats
            .get(&row.seat_code)
            .map_or(row.seat_code.as_str(), |facts| facts.seat_id.as_str());
        let account_id = final_account_ids
            .get(&row.domjudge_username)
            .ok_or(ImportStoreError::InvalidPersistedFacts)?;
        let inserted = diesel::insert_into(account_mappings::table)
            .values((
                account_mappings::seat_id.eq(seat_id),
                account_mappings::account_id.eq(account_id),
            ))
            .execute(connection)
            .map_err(|_| ImportStoreError::MutationFailed)?;
        if inserted != 1 {
            return Err(ImportStoreError::MutationConflict);
        }
    }
    Ok(())
}

fn persist_revisions(
    connection: &mut SqliteConnection,
    plan: &CommitPlan,
) -> Result<(), ImportStoreError> {
    if !plan.configuration_revision_advanced && !plan.binding_revision_advanced {
        return Ok(());
    }
    let updated = diesel::update(
        revision_counters::table
            .filter(revision_counters::singleton.eq(Some(1_i32)))
            .filter(
                revision_counters::configuration_revision.eq(diesel::dsl::sql::<Integer>("")
                    .bind::<BigInt, _>(plan.baseline_configuration_revision)),
            )
            .filter(revision_counters::binding_revision.eq(
                diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(plan.baseline_binding_revision),
            )),
    )
    .set(
        (
            revision_counters::configuration_revision
                .eq(diesel::dsl::sql::<Integer>("")
                    .bind::<BigInt, _>(plan.next_configuration_revision)),
            revision_counters::binding_revision
                .eq(diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(plan.next_binding_revision)),
        ),
    )
    .execute(connection)
    .map_err(|_| ImportStoreError::MutationFailed)?;
    if updated != 1 {
        return Err(ImportStoreError::MutationConflict);
    }
    Ok(())
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
    seat_id: String,
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
            seats::seat_id,
            seats::seat_code,
            accounts::domjudge_username.nullable(),
            device_bindings::device_pk.nullable(),
        ))
        .order(seats::seat_code)
        .load::<(String, String, Option<String>, Option<String>)>(connection)
        .map_err(|_| ImportStoreError::CurrentFactsReadFailed)?;

    let mut current = BTreeMap::new();
    for (seat_id, seat_code, current_domjudge_username, device_id) in rows {
        if current
            .insert(
                seat_code,
                CurrentSeatFacts {
                    seat_id,
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
    #[snafu(display("the import vault record could not be read"))]
    VaultReadFailed,
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
    #[snafu(display("the committed import mutation failed"))]
    MutationFailed,
    #[snafu(display("the committed import mutation changed concurrently"))]
    MutationConflict,
    #[snafu(display("an import revision could not be advanced"))]
    RevisionOverflow,
    #[snafu(display("an account credential revision could not be advanced"))]
    CredentialRevisionOverflow,
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
            | ImportStoreError::VaultReadFailed
            | ImportStoreError::RevisionsReadFailed
            | ImportStoreError::CurrentFactsReadFailed
            | ImportStoreError::PreviewSerializationFailed
            | ImportStoreError::VaultInsertFailed
            | ImportStoreError::ExpiryCalculationFailed
            | ImportStoreError::CandidateInsertFailed
            | ImportStoreError::AuditInsertFailed
            | ImportStoreError::MutationFailed
            | ImportStoreError::MutationConflict
            | ImportStoreError::RevisionOverflow
            | ImportStoreError::CredentialRevisionOverflow => Self::PersistenceFailure,
        }
    }
}

#[cfg(test)]
mod tests {
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
                account_mappings, accounts, audit_events, device_bindings,
                pending_import_candidate, revision_counters, seats, server_vault_records,
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
    async fn token_mismatch_audit_failure_preserves_unavailable_outcome() -> Result<(), TestFailure>
    {
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

    async fn full_rollback_snapshot(
        database: &Database,
    ) -> Result<FullRollbackSnapshot, TestFailure> {
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
                Ok::<FullRollbackSnapshot, diesel::result::Error>(FullRollbackSnapshot {
                    revisions,
                    seat_rows,
                    account_rows,
                    mapping_rows,
                    binding_rows,
                    vault_rows,
                    candidate_rows,
                    audit_rows,
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
}
