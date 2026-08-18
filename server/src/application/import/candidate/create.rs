use std::{collections::HashSet, path::Path};

use uuid::Uuid;

use crate::{
    audit::{AuditEvent, AuditEventId, CorrelationId},
    db::{self, Database},
    vault,
};

use super::super::{
    csv::{
        ACCOUNT_USERNAME_LENGTH_LIMIT, ImportRow, MAX_IMPORT_ROWS, PASSWORD_LENGTH_LIMIT,
        SEAT_CODE_LENGTH_LIMIT, parse_csv,
    },
    diff::compute_diff,
};
use super::{
    expire::expire_candidate,
    types::{
        CandidateExpiry, CandidateRecord, CreatedImportCandidate, ImportError,
        PendingImportCandidate, PreviewToken, SealedCommitRow,
    },
};

/// Strictly parses, encrypts, diffs, and atomically persists one import candidate.
///
/// # Errors
///
/// Returns a typed [`ImportError`] for invalid input, a live singleton candidate,
/// entropy failure, vault failure, or persistence failure.
pub(crate) async fn create_import_candidate(
    database: &Database,
    master_key_path: &Path,
    raw_csv: &[u8],
    correlation_id: CorrelationId,
) -> Result<CreatedImportCandidate, ImportError> {
    let parsed = parse_csv(raw_csv).map_err(ImportError::InvalidCsv)?;
    let preview_token = PreviewToken::generate()?;
    let preview_token_hash = preview_token.sha256();
    let candidate_rows = parsed.candidate_rows();
    let (nonce, ciphertext) = vault::seal(master_key_path, parsed.staging_plaintext())
        .map_err(|_| ImportError::VaultFailure)?;
    drop(parsed);

    let candidate_id = Uuid::now_v7();
    let payload_vault_record_id = Uuid::now_v7();
    let expiry_audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    let create_audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    database
        .write(move |transaction| {
            if let Some(pending) = db::import::pending_import_candidate::find(transaction)? {
                match pending.expiry() {
                    CandidateExpiry::Valid => return Err(ImportError::CandidatePending),
                    CandidateExpiry::Expired => expire_candidate(
                        transaction,
                        &pending,
                        correlation_id,
                        expiry_audit_event_id,
                    )?,
                }
            }

            let (baseline_configuration_revision, baseline_binding_revision) =
                db::import::revision_counters::read(transaction)
                    .map_err(ImportError::from_contest_persistence)?;
            let current_seats = db::import::query::read_current_seats(transaction)?;
            let diff = compute_diff(&current_seats, &candidate_rows)?;
            let expires_at = db::import::pending_import_candidate::calculate_expiry(transaction)?;
            let pending = PendingImportCandidate::new(
                candidate_id,
                expires_at,
                baseline_configuration_revision,
                baseline_binding_revision,
                diff,
            );
            let candidate = CandidateRecord::new(
                pending,
                preview_token_hash,
                payload_vault_record_id,
                CandidateExpiry::Valid,
            );

            let inserted_payload = db::import::server_vault_records::insert_import_payload(
                transaction,
                &candidate,
                &nonce,
                &ciphertext,
            )?;
            if inserted_payload != 1 {
                return Err(ImportError::PersistenceFailure);
            }
            let inserted_candidate =
                db::import::pending_import_candidate::insert(transaction, &candidate)?;
            if inserted_candidate != 1 {
                return Err(ImportError::PersistenceFailure);
            }

            let event = AuditEvent::import_candidate_created(
                create_audit_event_id,
                correlation_id,
                candidate_id,
                candidate.diff().seats_added().len(),
                candidate.diff().seats_removed().len(),
                candidate.diff().mappings_changed().len(),
                candidate.diff().binding_impacts().len(),
            );
            db::audit::insert(transaction, &event).map_err(ImportError::from_audit_persistence)?;
            Ok(candidate.into_created(preview_token))
        })
        .await
}

pub(crate) async fn audit_invalid_import_upload(
    database: &Database,
    correlation_id: CorrelationId,
) -> Result<(), ImportError> {
    let audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    database
        .write(move |transaction| {
            let event = AuditEvent::import_candidate_rejected(audit_event_id, correlation_id);
            db::audit::insert(transaction, &event).map_err(ImportError::from_audit_persistence)
        })
        .await
}

pub(in crate::application::import) fn decode_staging_rows(
    plaintext: &[u8],
) -> Result<Vec<ImportRow>, ImportError> {
    let rows = serde_json::from_slice::<Vec<ImportRow>>(plaintext)
        .map_err(|_| ImportError::PersistenceFailure)?;
    validate_staging_rows(&rows)?;
    Ok(rows)
}

fn validate_staging_rows(rows: &[ImportRow]) -> Result<(), ImportError> {
    if rows.is_empty() || rows.len() > MAX_IMPORT_ROWS {
        return Err(ImportError::PersistenceFailure);
    }
    let mut seat_codes = HashSet::with_capacity(rows.len());
    let mut account_usernames = HashSet::with_capacity(rows.len());
    for row in rows {
        if !valid_staged_field(&row.seat_code, SEAT_CODE_LENGTH_LIMIT)
            || !valid_staged_field(&row.domjudge_username, ACCOUNT_USERNAME_LENGTH_LIMIT)
            || !valid_staged_field(&row.password, PASSWORD_LENGTH_LIMIT)
            || !seat_codes.insert(row.seat_code.as_str())
            || !account_usernames.insert(row.domjudge_username.as_str())
        {
            return Err(ImportError::PersistenceFailure);
        }
    }
    Ok(())
}

fn valid_staged_field(value: &str, length_limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= length_limit
        && !value.contains(',')
        && !value.chars().any(char::is_control)
}

pub(in crate::application::import) fn seal_commit_rows(
    vault_session: &vault::VaultSession,
    rows: &[ImportRow],
) -> Result<Vec<SealedCommitRow>, ImportError> {
    let mut sealed_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let (nonce, ciphertext) = vault_session
            .seal(row.password.as_bytes())
            .map_err(|_| ImportError::VaultFailure)?;
        sealed_rows.push(SealedCommitRow {
            seat_code: row.seat_code.clone(),
            domjudge_username: row.domjudge_username.clone(),
            nonce,
            ciphertext,
        });
    }
    Ok(sealed_rows)
}
