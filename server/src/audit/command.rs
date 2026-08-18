use uuid::Uuid;

use super::{AuditDetail, AuditEvent, AuditEventId, CorrelationId};

impl AuditEvent {
    #[must_use]
    pub(crate) fn command_created(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        command_id: Uuid,
        group_correlation_id: Option<String>,
        kind: &'static str,
        payload_version: i32,
        request_fingerprint_version: i32,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "command_create",
            resource_type: "command",
            resource_id: Some(command_id.to_string()),
            result: "succeeded",
            reason_code: Some("operator_requested"),
            correlation_id,
            group_correlation_id,
            detail: AuditDetail::CommandCreated {
                kind,
                payload_version,
                request_fingerprint_version,
            },
        }
    }

    #[must_use]
    pub(crate) fn command_request_conflict(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        command_id: Uuid,
        group_correlation_id: Option<String>,
        request_fingerprint_version: i32,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "command_create",
            resource_type: "command",
            resource_id: Some(command_id.to_string()),
            result: "rejected",
            reason_code: Some("COMMAND_REQUEST_CONFLICT"),
            correlation_id,
            group_correlation_id,
            detail: AuditDetail::CommandRequestConflict {
                request_fingerprint_version,
            },
        }
    }

    #[must_use]
    pub(crate) fn command_terminal(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        command_id: Uuid,
        kind: String,
        terminal_state: &'static str,
        terminal_error_code: Option<String>,
    ) -> Self {
        let result = match terminal_state {
            "succeeded" => "succeeded",
            "cancelled" | "expired" => "noop",
            _ => "failed",
        };
        Self {
            id: audit_event_id,
            actor: "device:control",
            action_kind: "command_terminal",
            resource_type: "command",
            resource_id: Some(command_id.to_string()),
            result,
            reason_code: Some("device_reported"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::CommandTerminal {
                kind,
                terminal_state,
                terminal_error_code,
            },
        }
    }
}
