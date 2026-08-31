use crate::audit::{
    AuditDetail, AuditEvent, AuditEventId, CorrelationId, ProvisioningWindowAuditResult,
};

use super::ProvisioningWindowAction;

impl AuditEvent {
    pub(super) const fn operator_provisioning_window(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        action: ProvisioningWindowAction,
        result: ProvisioningWindowAuditResult,
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
            detail: AuditDetail::OperatorProvisioningWindow {},
        }
    }
}
