use uuid::Uuid;

use crate::{
    audit::{AuditEvent, AuditEventId, CorrelationId, EnrollmentDecisionAuditResult},
    db::{self, Database},
};

use super::{
    EnrollmentDecisionOutcome, EnrollmentDecisionState, EnrollmentError, EnrollmentRequestId,
    EnrollmentRequestStatus, EnrollmentResolution,
};

pub(super) async fn approve_request(
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
}

pub(super) async fn reject_request(
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
}

#[derive(Clone, Copy)]
enum EnrollmentDecision {
    Approve,
    Reject,
}

async fn mutate_pending_request(
    database: &Database,
    request_id: Uuid,
    correlation_id: CorrelationId,
    decision: EnrollmentDecision,
    audit_event_id: AuditEventId,
) -> Result<EnrollmentDecisionOutcome, EnrollmentError> {
    database
        .write(move |transaction| {
            let projection =
                db::device::enrollment::query::request_for_decision(transaction, request_id)?
                    .ok_or(EnrollmentError::RequestNotPending)?;
            let target_state = match decision {
                EnrollmentDecision::Approve => EnrollmentDecisionState::Approved,
                EnrollmentDecision::Reject => EnrollmentDecisionState::Rejected,
            };
            let audit_result = match (projection.state, target_state) {
                (EnrollmentRequestStatus::Pending, _) => EnrollmentDecisionAuditResult::Succeeded,
                (EnrollmentRequestStatus::Approved, EnrollmentDecisionState::Approved)
                | (EnrollmentRequestStatus::Rejected, EnrollmentDecisionState::Rejected) => {
                    EnrollmentDecisionAuditResult::Noop
                }
                _ => return Err(EnrollmentError::RequestNotPending),
            };
            if projection.resolution != Some(EnrollmentResolution::ReplaceDeviceCredentials)
                || projection.resolved_device_id.is_none()
                || projection.issuance_audit_event_id.is_some()
            {
                return Err(EnrollmentError::InvalidPersistedFacts);
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
            db::audit::insert_enrollment(transaction, &event)?;
            if matches!(audit_result, EnrollmentDecisionAuditResult::Succeeded) {
                db::device::enrollment::request::compare_and_swap_pending_to_decision(
                    transaction,
                    request_id,
                    target_state,
                )?;
            }
            Ok(EnrollmentDecisionOutcome {
                enrollment_request_id: request_id,
                state: target_state,
            })
        })
        .await
}
