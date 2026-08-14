use diesel::{ExpressionMethods, RunQueryDsl, dsl::sql, sql_types::Text, sqlite::SqliteConnection};
use serde::Serialize;
use snafu::Snafu;
use uuid::Uuid;

use crate::{application::contest::DeviceLifecycleAction, db::schema::audit_events};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditEventId(Uuid);

impl AuditEventId {
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    fn as_text(&self) -> String {
        self.0.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrelationId(Uuid);

impl CorrelationId {
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub(crate) fn as_text(&self) -> String {
        self.0.to_string()
    }
}

/// The event-specific evidence persisted as `redacted_detail_json`.
///
/// `untagged` keeps every variant a bare object with no discriminant, so a
/// zero-field variant encodes as `{}`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum AuditDetail {
    RecoveryClose {
        previous_revision: i64,
        new_revision: i64,
    },
    FirstAdminCreated {
        role: &'static str,
    },
    SessionEstablished {
        role: &'static str,
    },
    SessionTerminated {},
    SessionExpired {},
    DeviceLifecycle {
        resulting_state: &'static str,
        removed_token_count: i64,
        revoked_certificate_count: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceLifecycleAuditResult {
    Succeeded,
    Noop,
}

#[derive(Debug)]
pub struct AuditEvent {
    id: AuditEventId,
    actor: &'static str,
    action_kind: &'static str,
    resource_type: &'static str,
    resource_id: Option<String>,
    result: &'static str,
    reason_code: Option<&'static str>,
    correlation_id: CorrelationId,
    group_correlation_id: Option<&'static str>,
    detail: AuditDetail,
}

impl AuditEvent {
    #[must_use]
    pub const fn recovery_close(
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

    pub(crate) fn audit_event_id_text(&self) -> String {
        self.id.as_text()
    }
}

pub(crate) fn insert_diesel(
    connection: &mut SqliteConnection,
    event: &AuditEvent,
) -> Result<(), AuditError> {
    let detail_json = serde_json::to_string(&event.detail).unwrap_or_else(|_| {
        tracing::error!(
            correlation_id = %event.correlation_id.as_text(),
            "audit detail serialization invariant failed"
        );
        panic!("audit detail serialization invariant failed");
    });

    diesel::insert_into(audit_events::table)
        .values((
            audit_events::audit_event_id.eq(event.id.as_text()),
            audit_events::occurred_at.eq(sql::<Text>("strftime('%Y-%m-%dT%H:%M:%fZ', 'now')")),
            audit_events::actor.eq(event.actor),
            audit_events::action_kind.eq(event.action_kind),
            audit_events::resource_type.eq(event.resource_type),
            audit_events::resource_id.eq(event.resource_id.as_deref()),
            audit_events::result.eq(event.result),
            audit_events::reason_code.eq(event.reason_code),
            audit_events::correlation_id.eq(event.correlation_id.as_text()),
            audit_events::group_correlation_id.eq(event.group_correlation_id),
            audit_events::redacted_detail_json.eq(detail_json),
        ))
        .execute(connection)
        .map_err(|_| AuditError::InsertFailed)?;

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub enum AuditError {
    #[snafu(display("the audit event could not be persisted"))]
    InsertFailed,
}
