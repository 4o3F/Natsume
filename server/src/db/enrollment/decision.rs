use diesel::{OptionalExtension, RunQueryDsl, sql_types::Text};
use uuid::Uuid;

use crate::{
    application::enrollment::{
        EnrollmentDecisionOutcome, EnrollmentDecisionState, EnrollmentError, EnrollmentRequestId,
    },
    audit::{self, AuditEvent, AuditEventId, CorrelationId, EnrollmentDecisionAuditResult},
    db::Database,
};

use super::{EnrollmentStoreError, row::DecisionRequestRow};

pub(crate) async fn approve_request(
    database: &Database,
    request_id: &EnrollmentRequestId,
    correlation_id: CorrelationId,
) -> Result<EnrollmentDecisionOutcome, EnrollmentError> {
    mutate_pending_request(
        database,
        request_id.value(),
        correlation_id,
        EnrollmentDecision::Approve,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(EnrollmentError::from)
}

pub(crate) async fn reject_request(
    database: &Database,
    request_id: &EnrollmentRequestId,
    correlation_id: CorrelationId,
) -> Result<EnrollmentDecisionOutcome, EnrollmentError> {
    mutate_pending_request(
        database,
        request_id.value(),
        correlation_id,
        EnrollmentDecision::Reject,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(EnrollmentError::from)
}

#[derive(Clone, Copy)]
pub(super) enum EnrollmentDecision {
    Approve,
    Reject,
}

pub(super) async fn mutate_pending_request(
    database: &Database,
    request_id: Uuid,
    correlation_id: CorrelationId,
    decision: EnrollmentDecision,
    audit_event_id: AuditEventId,
) -> Result<EnrollmentDecisionOutcome, EnrollmentStoreError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                let row = diesel::sql_query(
                    "SELECT state, resolution, resolved_device_pk, issuance_audit_event_id \
                     FROM enrollment_requests WHERE enrollment_request_id = ?",
                )
                .bind::<Text, _>(request_id.to_string())
                .get_result::<DecisionRequestRow>(connection)
                .optional()
                .map_err(|_| EnrollmentStoreError::RequestReadFailed)?
                .ok_or(EnrollmentStoreError::RequestNotPending)?;
                let target_state = match decision {
                    EnrollmentDecision::Approve => EnrollmentDecisionState::Approved,
                    EnrollmentDecision::Reject => EnrollmentDecisionState::Rejected,
                };
                let audit_result = match row.state.as_str() {
                    "pending" => EnrollmentDecisionAuditResult::Succeeded,
                    current if current == target_state.as_persisted() => {
                        EnrollmentDecisionAuditResult::Noop
                    }
                    _ => return Err(EnrollmentStoreError::RequestNotPending),
                };
                if row.resolution.as_deref() != Some("replace_device_credentials")
                    || row.resolved_device_pk.is_none()
                    || row.issuance_audit_event_id.is_some()
                {
                    return Err(EnrollmentStoreError::InvalidPersistedFacts);
                }
                let event = match decision {
                    EnrollmentDecision::Approve => AuditEvent::enrollment_request_approved(
                        audit_event_id,
                        correlation_id,
                        request_id,
                        audit_result,
                    ),
                    EnrollmentDecision::Reject => AuditEvent::enrollment_request_rejected(
                        audit_event_id,
                        correlation_id,
                        request_id,
                        audit_result,
                    ),
                };
                audit::insert_diesel(connection, &event)
                    .map_err(|_| EnrollmentStoreError::AuditInsertFailed)?;
                if matches!(audit_result, EnrollmentDecisionAuditResult::Succeeded) {
                    let updated = diesel::sql_query(
                        "UPDATE enrollment_requests SET state = ? \
                         WHERE enrollment_request_id = ? AND state = 'pending'",
                    )
                    .bind::<Text, _>(target_state.as_persisted())
                    .bind::<Text, _>(request_id.to_string())
                    .execute(connection)
                    .map_err(|_| EnrollmentStoreError::RequestMutationFailed)?;
                    if updated != 1 {
                        return Err(EnrollmentStoreError::CompareAndSwapConflict);
                    }
                }
                Ok(EnrollmentDecisionOutcome {
                    enrollment_request_id: request_id,
                    state: target_state,
                })
            })
        })
        .await
        .map_err(|_| EnrollmentStoreError::AcquireFailed)?
}
