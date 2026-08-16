use crate::application::contest::DeviceLifecycleAction;

use super::{AuditDetail, AuditEvent, AuditEventId, CorrelationId, DeviceLifecycleAuditResult};

impl AuditEvent {
    #[must_use]
    pub(crate) fn device_lifecycle(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        device_id: String,
        action: DeviceLifecycleAction,
        result: DeviceLifecycleAuditResult,
        detail: AuditDetail,
    ) -> Self {
        let action_kind = match action {
            DeviceLifecycleAction::Revoke => "revoke_device",
            DeviceLifecycleAction::Disable => "disable_device",
        };
        let (result, reason_code) = match result {
            DeviceLifecycleAuditResult::Succeeded => ("succeeded", "operator_requested"),
            DeviceLifecycleAuditResult::Noop => ("noop", "target_already_satisfied"),
        };
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind,
            resource_type: "device",
            resource_id: Some(device_id),
            result,
            reason_code: Some(reason_code),
            correlation_id,
            group_correlation_id: None,
            detail,
        }
    }
}
