use uuid::Uuid;

use crate::application::device::enrollment::EnrollmentResolution;

use super::super::{
    AuditDetail, AuditEvent, AuditEventId, CorrelationId, DeviceCredentialsIssuedAuditFacts,
    EnrollmentDecisionAuditResult, EnrollmentExpiryActor,
};

impl AuditEvent {
    #[must_use]
    pub(crate) fn enrollment_request_created(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        enrollment_request_id: Uuid,
        resolution: EnrollmentResolution,
        gateway_spki_sha256: [u8; 32],
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "device:enrollment",
            action_kind: "create_enrollment_request",
            resource_type: "enrollment_request",
            resource_id: Some(enrollment_request_id.to_string()),
            result: "succeeded",
            reason_code: Some("credential_replacement"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::EnrollmentRequestCreated {
                resolution: resolution.as_persisted(),
                state: "pending",
                gateway_spki_sha256: hex::encode(gateway_spki_sha256),
            },
        }
    }

    #[must_use]
    pub(crate) fn device_credentials_issued(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        enrollment_request_id: Uuid,
        facts: DeviceCredentialsIssuedAuditFacts,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "device:enrollment",
            action_kind: "issue_device_credentials",
            resource_type: "enrollment_request",
            resource_id: Some(enrollment_request_id.to_string()),
            result: "succeeded",
            reason_code: Some(facts.reason.as_audit_reason()),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::DeviceCredentialsIssued {
                resolution: facts.resolution.as_persisted(),
                certificate_serial: facts.certificate_serial,
                gateway_spki_sha256: hex::encode(facts.gateway_spki_sha256),
                previous_device_state: facts.previous_device_state,
                evicted_live_connection: facts.evicted_live_connection,
            },
        }
    }

    #[must_use]
    pub(crate) fn enrollment_request_approved(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        enrollment_request_id: Uuid,
        audit_result: EnrollmentDecisionAuditResult,
    ) -> Self {
        let (result, reason_code) = match audit_result {
            EnrollmentDecisionAuditResult::Succeeded => ("succeeded", "operator_requested"),
            EnrollmentDecisionAuditResult::Noop => ("noop", "target_already_satisfied"),
        };
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "approve_enrollment_request",
            resource_type: "enrollment_request",
            resource_id: Some(enrollment_request_id.to_string()),
            result,
            reason_code: Some(reason_code),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::EnrollmentRequestApproved {},
        }
    }

    #[must_use]
    pub(crate) fn enrollment_request_rejected(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        enrollment_request_id: Uuid,
        audit_result: EnrollmentDecisionAuditResult,
    ) -> Self {
        let (result, reason_code) = match audit_result {
            EnrollmentDecisionAuditResult::Succeeded => ("succeeded", "operator_requested"),
            EnrollmentDecisionAuditResult::Noop => ("noop", "target_already_satisfied"),
        };
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "reject_enrollment_request",
            resource_type: "enrollment_request",
            resource_id: Some(enrollment_request_id.to_string()),
            result,
            reason_code: Some(reason_code),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::EnrollmentRequestRejected {},
        }
    }

    #[must_use]
    pub(crate) const fn enrollment_requests_expired(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        actor: EnrollmentExpiryActor,
        expired_count: i64,
    ) -> Self {
        let actor = match actor {
            EnrollmentExpiryActor::Operator => "operator:self",
            EnrollmentExpiryActor::Recovery => "system:recovery",
        };
        Self {
            id: audit_event_id,
            actor,
            action_kind: "expire_enrollment_requests",
            resource_type: "enrollment_request_set",
            resource_id: None,
            result: "succeeded",
            reason_code: Some("window_closed"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::EnrollmentRequestsExpired { expired_count },
        }
    }
}
