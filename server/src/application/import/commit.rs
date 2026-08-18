use std::path::Path;

use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    audit::{AuditEvent, AuditEventId, CorrelationId, ImportCommitRejectionReason},
    db::{self, Database},
    vault,
};

mod apply;
mod plan;

pub(crate) use self::apply::NewAccountFacts;
use self::{apply::apply_commit_plan, plan::prepare_commit_plan};
use super::{
    CommittedImportFacts, ImportError, PreviewToken, SealedCommitRow,
    candidate::{
        CandidateExpiry, audit_preview_token_mismatch, decode_staging_rows, expire_candidate,
        read_commit_candidate, seal_commit_rows,
    },
};

struct CommitRequest {
    candidate_id: Uuid,
    expected_preview_token_hash: [u8; 32],
    expected_payload_vault_record_id: Uuid,
    sealed_rows: Vec<SealedCommitRow>,
    correlation_id: CorrelationId,
}

pub(crate) async fn commit_import(
    database: &Database,
    master_key_path: &Path,
    import_id: Uuid,
    presented_token: &PreviewToken,
    correlation_id: CorrelationId,
) -> Result<CommittedImportFacts, ImportError> {
    let payload = read_commit_candidate(database, import_id, correlation_id).await?;
    let presented_token_hash = presented_token.sha256();
    if !bool::from(
        presented_token_hash
            .as_slice()
            .ct_eq(payload.preview_token_hash().as_slice()),
    ) {
        audit_preview_token_mismatch(
            database,
            payload.candidate_id(),
            *payload.preview_token_hash(),
            correlation_id,
        )
        .await?;
        return Err(ImportError::CandidateUnavailable);
    }

    let vault_session = vault::load(master_key_path).map_err(|_| ImportError::VaultFailure)?;
    let plaintext = vault_session
        .open(payload.nonce(), payload.ciphertext())
        .map_err(|_| ImportError::VaultFailure)?;
    let rows = decode_staging_rows(&plaintext)?;
    drop(plaintext);
    let sealed_rows = seal_commit_rows(&vault_session, &rows)?;
    drop(rows);
    drop(vault_session);
    let request = CommitRequest {
        candidate_id: payload.candidate_id(),
        expected_preview_token_hash: *payload.preview_token_hash(),
        expected_payload_vault_record_id: payload.payload_vault_record_id(),
        sealed_rows,
        correlation_id,
    };

    let expiry_audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    let commit_audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    let outcome = database
        .write(move |transaction| {
            commit_import_in_transaction(
                transaction,
                &request,
                expiry_audit_event_id,
                commit_audit_event_id,
            )
        })
        .await?;

    match outcome {
        CommitOutcome::Committed(facts) => Ok(facts),
        CommitOutcome::Unavailable => Err(ImportError::CandidateUnavailable),
        CommitOutcome::Stale => Err(ImportError::PreviewStale),
    }
}

fn commit_import_in_transaction(
    transaction: &mut db::Transaction<'_>,
    request: &CommitRequest,
    expiry_audit_event_id: AuditEventId,
    commit_audit_event_id: AuditEventId,
) -> Result<CommitOutcome, ImportError> {
    let Some(candidate) = db::import::pending_import_candidate::find(transaction)? else {
        return Ok(CommitOutcome::Unavailable);
    };
    if candidate.candidate_id() != request.candidate_id {
        return Ok(CommitOutcome::Unavailable);
    }
    if candidate.expiry() == CandidateExpiry::Expired {
        expire_candidate(
            transaction,
            &candidate,
            request.correlation_id,
            expiry_audit_event_id,
        )?;
        return Ok(CommitOutcome::Unavailable);
    }
    if candidate.preview_token_hash() != &request.expected_preview_token_hash
        || candidate.payload_vault_record_id() != request.expected_payload_vault_record_id
    {
        return Ok(CommitOutcome::Unavailable);
    }

    let (configuration_revision, binding_revision) =
        db::import::revision_counters::read(transaction)?;
    if configuration_revision != candidate.baseline_configuration_revision()
        || binding_revision != candidate.baseline_binding_revision()
    {
        let event = AuditEvent::import_commit_rejected(
            commit_audit_event_id,
            request.correlation_id,
            request.candidate_id,
            ImportCommitRejectionReason::BaselineStale,
        );
        // The stale classification is externally frozen even when its audit fails.
        if db::audit::insert_import(transaction, &event).is_err() {
            tracing::warn!(
                discriminant = "baseline_stale_audit_write_failed",
                "import rejection audit write failed"
            );
        }
        return Ok(CommitOutcome::Stale);
    }

    let current_seats = db::import::query::read_current_seats(transaction)?;
    let current_accounts = db::import::query::read_current_accounts(transaction)?;
    let plan = prepare_commit_plan(
        current_seats,
        current_accounts,
        &request.sealed_rows,
        candidate.baseline_configuration_revision(),
        candidate.baseline_binding_revision(),
    )?;
    let mutation = apply_commit_plan(transaction, &request.sealed_rows, &plan)?;
    let removed_candidate =
        db::import::pending_import_candidate::delete_exact(transaction, &candidate)?;
    let removed_payload =
        db::import::server_vault_records::delete_import_payload(transaction, &candidate)?;
    if removed_candidate != 1 || removed_payload != 1 {
        return Err(ImportError::PersistenceFailure);
    }

    let event = AuditEvent::import_committed(
        commit_audit_event_id,
        request.correlation_id,
        request.candidate_id,
        &mutation.audit_facts(),
    );
    db::audit::insert_import(transaction, &event)?;
    Ok(CommitOutcome::Committed(CommittedImportFacts::new(
        mutation.configuration_revision,
        mutation.binding_revision,
    )))
}

enum CommitOutcome {
    Committed(CommittedImportFacts),
    Unavailable,
    Stale,
}
