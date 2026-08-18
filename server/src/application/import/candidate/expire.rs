use uuid::Uuid;

use crate::{
    audit::{AuditEvent, AuditEventId, CorrelationId, ImportCommitRejectionReason},
    db::{self, Database},
};

use super::types::{
    CandidateExpiry, CandidateRecord, CommitCandidatePayload, ImportError, PendingImportCandidate,
};

pub(crate) async fn read_pending_import_candidate(
    database: &Database,
    correlation_id: CorrelationId,
) -> Result<Option<PendingImportCandidate>, ImportError> {
    read_pending_import_candidate_after_expired_observation(database, correlation_id, || {}).await
}

pub(in crate::application::import) async fn read_pending_import_candidate_after_expired_observation<
    F,
>(
    database: &Database,
    correlation_id: CorrelationId,
    expired_observed: F,
) -> Result<Option<PendingImportCandidate>, ImportError>
where
    F: FnOnce() + Send,
{
    let observed = database
        .read(db::import::pending_import_candidate::find)
        .await?;
    let Some(observed) = observed else {
        return Ok(None);
    };
    if observed.expiry() == CandidateExpiry::Valid {
        return Ok(Some(observed.into_pending()));
    }
    expired_observed();

    cleanup_expired_pending_observation(
        database,
        observed.candidate_id(),
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
}

pub(crate) async fn discard_import(
    database: &Database,
    import_id: Uuid,
    correlation_id: CorrelationId,
) -> Result<(), ImportError> {
    let expiry_audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    let discard_audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    let outcome = database
        .write(move |transaction| {
            let Some(candidate) = db::import::pending_import_candidate::find(transaction)? else {
                return Ok(DiscardOutcome::Unavailable);
            };
            if candidate.candidate_id() != import_id {
                return Ok(DiscardOutcome::Unavailable);
            }
            if candidate.expiry() == CandidateExpiry::Expired {
                expire_candidate(
                    transaction,
                    &candidate,
                    correlation_id,
                    expiry_audit_event_id,
                )?;
                return Ok(DiscardOutcome::Unavailable);
            }

            let payload_was_present =
                db::import::server_vault_records::read_import_payload(transaction, &candidate)?
                    .is_some();
            let removed_candidate =
                db::import::pending_import_candidate::delete_exact(transaction, &candidate)?;
            if removed_candidate != 1 {
                return Err(ImportError::PersistenceFailure);
            }
            let removed_payload =
                db::import::server_vault_records::delete_import_payload(transaction, &candidate)?;
            if removed_payload != usize::from(payload_was_present) {
                return Err(ImportError::PersistenceFailure);
            }
            let event = AuditEvent::import_candidate_discarded(
                discard_audit_event_id,
                correlation_id,
                import_id,
            );
            db::audit::insert_import(transaction, &event)?;
            Ok(DiscardOutcome::Discarded)
        })
        .await?;
    match outcome {
        DiscardOutcome::Discarded => Ok(()),
        DiscardOutcome::Unavailable => Err(ImportError::CandidateUnavailable),
    }
}

pub(in crate::application::import) async fn read_commit_candidate(
    database: &Database,
    candidate_id: Uuid,
    correlation_id: CorrelationId,
) -> Result<CommitCandidatePayload, ImportError> {
    let observed = database
        .read(move |transaction| read_commit_candidate_in_transaction(transaction, candidate_id))
        .await?;
    match observed {
        CandidateReadOutcome::Available(payload) => Ok(*payload),
        CandidateReadOutcome::Unavailable => Err(ImportError::CandidateUnavailable),
        CandidateReadOutcome::Expired => {
            let expiry_audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
            let reread = database
                .write(
                    move |transaction| -> Result<CandidateReadOutcome, ImportError> {
                        match read_commit_candidate_in_transaction(transaction, candidate_id)? {
                            CandidateReadOutcome::Available(payload) => {
                                Ok(CandidateReadOutcome::Available(payload))
                            }
                            CandidateReadOutcome::Unavailable => {
                                Ok(CandidateReadOutcome::Unavailable)
                            }
                            CandidateReadOutcome::Expired => {
                                let candidate =
                                    db::import::pending_import_candidate::find(transaction)?
                                        .ok_or(ImportError::PersistenceFailure)?;
                                if candidate.candidate_id() != candidate_id {
                                    return Ok(CandidateReadOutcome::Unavailable);
                                }
                                expire_candidate(
                                    transaction,
                                    &candidate,
                                    correlation_id,
                                    expiry_audit_event_id,
                                )?;
                                Ok(CandidateReadOutcome::Unavailable)
                            }
                        }
                    },
                )
                .await?;
            match reread {
                CandidateReadOutcome::Available(payload) => Ok(*payload),
                CandidateReadOutcome::Expired | CandidateReadOutcome::Unavailable => {
                    Err(ImportError::CandidateUnavailable)
                }
            }
        }
    }
}

pub(in crate::application::import) async fn audit_preview_token_mismatch(
    database: &Database,
    candidate_id: Uuid,
    expected_preview_token_hash: [u8; 32],
    correlation_id: CorrelationId,
) -> Result<(), ImportError> {
    let expiry_audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    let rejection_audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    database
        .write(move |transaction| {
            let Some(candidate) = db::import::pending_import_candidate::find(transaction)? else {
                return Ok(());
            };
            if candidate.candidate_id() != candidate_id {
                return Ok(());
            }
            if candidate.expiry() == CandidateExpiry::Expired {
                return expire_candidate(
                    transaction,
                    &candidate,
                    correlation_id,
                    expiry_audit_event_id,
                );
            }
            if candidate.preview_token_hash() != &expected_preview_token_hash {
                return Ok(());
            }

            let event = AuditEvent::import_commit_rejected(
                rejection_audit_event_id,
                correlation_id,
                candidate_id,
                ImportCommitRejectionReason::PreviewTokenMismatch,
            );
            // A failed rejection audit must not become an existence oracle.
            if db::audit::insert_import(transaction, &event).is_err() {
                tracing::warn!(
                    discriminant = "preview_token_mismatch_audit_write_failed",
                    "import rejection audit write failed"
                );
            }
            Ok(())
        })
        .await
}

async fn cleanup_expired_pending_observation(
    database: &Database,
    observed_candidate_id: Uuid,
    correlation_id: CorrelationId,
    expiry_audit_event_id: AuditEventId,
) -> Result<Option<PendingImportCandidate>, ImportError> {
    database
        .write(move |transaction| {
            let Some(current) = db::import::pending_import_candidate::find(transaction)? else {
                return Ok(None);
            };
            if current.candidate_id() != observed_candidate_id {
                return match current.expiry() {
                    CandidateExpiry::Valid => Ok(Some(current.into_pending())),
                    CandidateExpiry::Expired => Ok(None),
                };
            }
            if current.expiry() == CandidateExpiry::Valid {
                return Ok(Some(current.into_pending()));
            }
            expire_candidate(transaction, &current, correlation_id, expiry_audit_event_id)?;
            Ok(None)
        })
        .await
}

fn read_commit_candidate_in_transaction(
    transaction: &mut db::Transaction<'_>,
    candidate_id: Uuid,
) -> Result<CandidateReadOutcome, ImportError> {
    let Some(candidate) = db::import::pending_import_candidate::find(transaction)? else {
        return Ok(CandidateReadOutcome::Unavailable);
    };
    if candidate.candidate_id() != candidate_id {
        return Ok(CandidateReadOutcome::Unavailable);
    }
    if candidate.expiry() == CandidateExpiry::Expired {
        return Ok(CandidateReadOutcome::Expired);
    }
    let payload = db::import::server_vault_records::read_import_payload(transaction, &candidate)?
        .ok_or(ImportError::PersistenceFailure)?;
    Ok(CandidateReadOutcome::Available(Box::new(
        CommitCandidatePayload::new(candidate, payload),
    )))
}

pub(in crate::application::import) fn expire_candidate(
    transaction: &mut db::Transaction<'_>,
    candidate: &CandidateRecord,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), ImportError> {
    let payload_was_present =
        db::import::server_vault_records::read_import_payload(transaction, candidate)?.is_some();
    let removed_candidate =
        db::import::pending_import_candidate::delete_exact(transaction, candidate)?;
    if removed_candidate != 1 {
        return Err(ImportError::PersistenceFailure);
    }
    let removed_payload =
        db::import::server_vault_records::delete_import_payload(transaction, candidate)?;
    if removed_payload != usize::from(payload_was_present) {
        return Err(ImportError::PersistenceFailure);
    }
    let event = AuditEvent::import_candidate_expired(
        audit_event_id,
        correlation_id,
        candidate.candidate_id(),
    );
    db::audit::insert_import(transaction, &event)
}

enum CandidateReadOutcome {
    Available(Box<CommitCandidatePayload>),
    Expired,
    Unavailable,
}

enum DiscardOutcome {
    Discarded,
    Unavailable,
}
