use crate::application::provisioning::ProvisioningWindowAction;

use super::{AuditDetail, AuditEvent, AuditEventId, CorrelationId, ProvisioningWindowAuditResult};

impl AuditEvent {
    #[must_use]
    pub(crate) const fn recovery_close(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        previous_revision: i64,
        new_revision: i64,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "system:recovery",
            action_kind: "close_provisioning_window",
            resource_type: "provisioning_window",
            resource_id: None,
            result: "succeeded",
            reason_code: Some("startup_recovery"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::RecoveryClose {
                previous_revision,
                new_revision,
            },
        }
    }

    #[must_use]
    pub(crate) const fn operator_provisioning_window(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        action: ProvisioningWindowAction,
        result: ProvisioningWindowAuditResult,
        previous_revision: i64,
        new_revision: i64,
    ) -> Self {
        let action_kind = match action {
            ProvisioningWindowAction::Open => "open_provisioning_window",
            ProvisioningWindowAction::Close => "close_provisioning_window",
        };
        let (result, reason_code) = match result {
            ProvisioningWindowAuditResult::Succeeded => ("succeeded", "operator_requested"),
            ProvisioningWindowAuditResult::Noop => ("noop", "target_already_satisfied"),
        };
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind,
            resource_type: "provisioning_window",
            resource_id: None,
            result,
            reason_code: Some(reason_code),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::OperatorProvisioningWindow {
                previous_revision,
                new_revision,
            },
        }
    }
}
