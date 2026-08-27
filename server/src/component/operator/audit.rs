use uuid::Uuid;

use crate::audit::{AuditDetail, AuditEvent, AuditEventId, CorrelationId};

impl AuditEvent {
    #[must_use]
    pub(crate) fn first_admin_created(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        operator_id: Uuid,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "system:bootstrap",
            action_kind: "create_first_admin",
            resource_type: "operator_account",
            resource_id: Some(operator_id.to_string()),
            result: "succeeded",
            reason_code: Some("initial_provisioning"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::FirstAdminCreated { role: "admin" },
        }
    }

    #[must_use]
    pub(crate) fn operator_password_reset(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        operator_id: Uuid,
        removed_session_count: usize,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "system:password-reset",
            action_kind: "reset_operator_password",
            resource_type: "operator_account",
            resource_id: Some(operator_id.to_string()),
            result: "succeeded",
            reason_code: Some("credential_recovery"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::OperatorPasswordReset {
                removed_session_count,
            },
        }
    }

    #[must_use]
    pub(crate) fn session_established(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        operator_id: Uuid,
        role: &'static str,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "establish_session",
            resource_type: "operator_session",
            resource_id: Some(operator_id.to_string()),
            result: "succeeded",
            reason_code: Some("credentials_verified"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::SessionEstablished { role },
        }
    }

    #[must_use]
    pub(crate) fn session_terminated(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        operator_id: Uuid,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "terminate_session",
            resource_type: "operator_session",
            resource_id: Some(operator_id.to_string()),
            result: "succeeded",
            reason_code: Some("operator_requested"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::SessionTerminated {},
        }
    }

    #[must_use]
    pub(crate) fn session_expired(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        operator_id: Uuid,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "system:expiry",
            action_kind: "expire_session",
            resource_type: "operator_session",
            resource_id: Some(operator_id.to_string()),
            result: "succeeded",
            reason_code: Some("absolute_expiry_observed"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::SessionExpired {},
        }
    }
}
