use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, QueryableByName, RunQueryDsl,
    sql_types::{BigInt, Integer, Text},
    sqlite::SqliteConnection,
};
use uuid::Uuid;

use crate::{
    application::import::{CandidateRowFacts, CommitCandidatePayload, ImportError},
    audit::{self, AuditEvent, AuditEventId, CorrelationId, ImportCommitRejectionReason},
    db::{
        Database,
        schema::{pending_import_candidate, server_vault_records},
    },
    vault::VaultRecordType,
};

use super::{
    CreatedCandidateFacts, ImportStoreError, PendingCandidateFacts, canonical_uuid_v7,
    diff::{compute_diff, read_current_seats, read_revision_counters},
    import_expiry,
};

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

pub(super) async fn audit_invalid_import_upload_with_id(
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

pub(super) struct CandidateCreationRequest {
    pub(super) candidate_rows: Vec<CandidateRowFacts>,
    pub(super) preview_token_hash: [u8; 32],
    pub(super) nonce: [u8; 24],
    pub(super) ciphertext: Vec<u8>,
    pub(super) correlation_id: CorrelationId,
}

pub(super) async fn create_import_candidate_with_ids(
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

pub(super) struct CandidateCreationInput {
    pub(super) request: CandidateCreationRequest,
    pub(super) candidate_id: Uuid,
    pub(super) payload_vault_record_id: Uuid,
    pub(super) expiry_audit_event_id: AuditEventId,
    pub(super) create_audit_event_id: AuditEventId,
}

pub(super) fn create_import_candidate_in_transaction(
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

pub(crate) async fn read_pending_import_candidate(
    database: &Database,
    correlation_id: CorrelationId,
) -> Result<Option<PendingCandidateFacts>, ImportError> {
    read_pending_import_candidate_with_id(
        database,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(ImportError::from)
}

pub(super) async fn read_pending_import_candidate_with_id(
    database: &Database,
    correlation_id: CorrelationId,
    expiry_audit_event_id: AuditEventId,
) -> Result<Option<PendingCandidateFacts>, ImportStoreError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                let Some(pending) = read_pending_candidate(connection)? else {
                    return Ok(None);
                };
                let candidate_id = canonical_uuid_v7(&pending.candidate_id)?;
                match pending.expiry_state {
                    1 => {
                        expire_pending_candidate_tolerant(
                            connection,
                            &pending,
                            candidate_id,
                            correlation_id,
                            expiry_audit_event_id,
                        )?;
                        Ok(None)
                    }
                    0 => {
                        let diff = serde_json::from_str(&pending.redacted_preview_json)
                            .map_err(|_| ImportStoreError::InvalidPersistedFacts)?;
                        Ok(Some(PendingCandidateFacts {
                            candidate_id,
                            expires_at: pending.expires_at,
                            baseline_configuration_revision: pending
                                .baseline_configuration_revision,
                            baseline_binding_revision: pending.baseline_binding_revision,
                            diff,
                        }))
                    }
                    _ => Err(ImportStoreError::InvalidPersistedFacts),
                }
            })
        })
        .await
        .map_err(|_| ImportStoreError::AcquireFailed)?
}

pub(super) enum CandidateReadOutcome {
    Available(CommitCandidatePayload),
    Unavailable,
}

pub(super) async fn read_commit_candidate_with_ids(
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

pub(super) async fn audit_preview_token_mismatch_with_ids(
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

#[derive(QueryableByName)]
pub(super) struct PendingCandidateRow {
    #[diesel(sql_type = Text)]
    pub(super) candidate_id: String,
    #[diesel(sql_type = Text)]
    pub(super) expires_at: String,
    #[diesel(sql_type = Text)]
    pub(super) payload_vault_record_id: String,
    #[diesel(sql_type = BigInt)]
    pub(super) baseline_configuration_revision: i64,
    #[diesel(sql_type = BigInt)]
    pub(super) baseline_binding_revision: i64,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    pub(super) preview_token_hash: Vec<u8>,
    #[diesel(sql_type = Text)]
    pub(super) redacted_preview_json: String,
    #[diesel(sql_type = BigInt)]
    pub(super) expiry_state: i64,
}

pub(super) fn read_pending_candidate(
    connection: &mut SqliteConnection,
) -> Result<Option<PendingCandidateRow>, ImportStoreError> {
    diesel::sql_query(
        "SELECT candidate_id, expires_at, payload_vault_record_id, \
         baseline_configuration_revision, baseline_binding_revision, preview_token_hash, \
         redacted_preview_json, \
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

pub(super) fn pending_preview_token_hash(
    pending: &PendingCandidateRow,
) -> Result<[u8; 32], ImportStoreError> {
    pending
        .preview_token_hash
        .as_slice()
        .try_into()
        .map_err(|_| ImportStoreError::InvalidPersistedFacts)
}

pub(super) fn expire_pending_candidate_tolerant(
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

pub(super) fn delete_candidate_and_optional_payload(
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
