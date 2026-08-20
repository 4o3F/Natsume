#![allow(dead_code)]

use serde::Serialize;
use snafu::Snafu;
use uuid::Uuid;

#[cfg(test)]
use crate::db::schema::audit_events;
#[cfg(test)]
use diesel::{ExpressionMethods, RunQueryDsl, dsl::sql, sql_types::Text, sqlite::SqliteConnection};

mod command;
mod import;
mod operator;
mod provisioning;

/// Redacted persistence boundary shared by the Audit adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
#[snafu(module)]
pub(crate) enum AuditPersistenceError {
    #[snafu(display("audit persistence failed"))]
    PersistenceFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuditEventId(Uuid);

impl AuditEventId {
    #[must_use]
    pub(crate) const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub(crate) fn as_text(&self) -> String {
        self.0.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CorrelationId(Uuid);

impl CorrelationId {
    #[must_use]
    pub(crate) const fn from_uuid(value: Uuid) -> Self {
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
pub(crate) enum AuditDetail {
    RecoveryClose {
        previous_revision: i64,
        new_revision: i64,
    },
    OperatorProvisioningWindow {
        previous_revision: i64,
        new_revision: i64,
    },
    FirstAdminCreated {
        role: &'static str,
    },
    OperatorPasswordReset {
        removed_session_count: usize,
    },
    SessionEstablished {
        role: &'static str,
    },
    SessionTerminated {},
    SessionExpired {},
    ImportCandidateCreated {
        seats_added_count: usize,
        seats_removed_count: usize,
        mappings_changed_count: usize,
        binding_impact_count: usize,
    },
    ImportCandidateRejected {},
    ImportCandidateExpired {},
    ImportCommitted {
        seats_added_count: usize,
        seats_removed_count: usize,
        mappings_changed_count: usize,
        binding_impact_count: usize,
        credential_revision_advanced_count: usize,
        configuration_revision_advanced: bool,
        binding_revision_advanced: bool,
    },
    ImportCommitRejected {},
    ImportCandidateDiscarded {},
    DeviceLifecycle {
        resulting_state: &'static str,
        removed_token_count: i64,
        revoked_certificate_count: i64,
    },
    EnrollmentRequestCreated {
        resolution: &'static str,
        state: &'static str,
        gateway_spki_sha256: String,
    },
    DeviceCredentialsIssued {
        resolution: &'static str,
        certificate_serial: String,
        gateway_spki_sha256: String,
        previous_device_state: Option<&'static str>,
        evicted_live_connection: bool,
    },
    EnrollmentRequestApproved {},
    EnrollmentRequestRejected {},
    EnrollmentRequestsExpired {
        expired_count: i64,
    },
    CommandCreated {
        kind: &'static str,
        payload_version: i32,
        request_fingerprint_version: i32,
    },
    CommandRequestConflict {
        request_fingerprint_version: i32,
    },
    CommandTerminal {
        kind: String,
        terminal_state: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        terminal_error_code: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceLifecycleAuditResult {
    Succeeded,
    Noop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProvisioningWindowAuditResult {
    Succeeded,
    Noop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentExpiryActor {
    Operator,
    Recovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentDecisionAuditResult {
    Succeeded,
    Noop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportCommitRejectionReason {
    PreviewTokenMismatch,
    BaselineStale,
}

impl ImportCommitRejectionReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PreviewTokenMismatch => "preview_token_mismatch",
            Self::BaselineStale => "baseline_stale",
        }
    }
}

pub(crate) struct ImportCommitAuditFacts {
    pub(crate) seats_added_count: usize,
    pub(crate) seats_removed_count: usize,
    pub(crate) mappings_changed_count: usize,
    pub(crate) binding_impact_count: usize,
    pub(crate) credential_revision_advanced_count: usize,
    pub(crate) configuration_revision_advanced: bool,
    pub(crate) binding_revision_advanced: bool,
}

#[derive(Debug)]
pub(crate) struct AuditEvent {
    pub(super) id: AuditEventId,
    pub(super) actor: &'static str,
    pub(super) action_kind: &'static str,
    pub(super) resource_type: &'static str,
    pub(super) resource_id: Option<String>,
    pub(super) result: &'static str,
    pub(super) reason_code: Option<&'static str>,
    pub(super) correlation_id: CorrelationId,
    pub(super) group_correlation_id: Option<String>,
    pub(super) detail: AuditDetail,
}

impl AuditEvent {
    pub(crate) fn audit_event_id_text(&self) -> String {
        self.id.as_text()
    }
}

#[cfg(test)]
pub(crate) fn insert_diesel(
    connection: &mut SqliteConnection,
    event: &AuditEvent,
) -> Result<(), AuditPersistenceError> {
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
            audit_events::group_correlation_id.eq(event.group_correlation_id.as_deref()),
            audit_events::redacted_detail_json.eq(detail_json),
        ))
        .execute(connection)
        .map_err(|_| AuditPersistenceError::PersistenceFailed)?;

    Ok(())
}
