use uuid::Uuid;

use crate::{
    application::import::ImportError,
    audit::{self, AuditEvent, AuditEventId, CorrelationId},
    db::Database,
};

use super::{
    ImportStoreError,
    candidate::{
        delete_candidate_and_optional_payload, expire_pending_candidate_tolerant,
        read_pending_candidate,
    },
};

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

pub(super) enum DiscardOutcome {
    Discarded,
    Unavailable,
}

pub(super) async fn discard_import_with_ids(
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
