use diesel::sqlite::SqliteConnection;
use diesel::{ExpressionMethods, RunQueryDsl, dsl::sql, sql_types::BigInt};
use serde::Serialize;
use snafu::Snafu;
use uuid::Uuid;

use crate::{db::Transaction, schema::audit_events};

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
    OperatorProvisioningWindow {},
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
    },
    ImportCommitRejected {},
    ImportCandidateDiscarded {},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProvisioningWindowAuditResult {
    Succeeded,
    Noop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportCommitRejectionReason {
    PreviewTokenMismatch,
    CandidateChanged,
    BaselineStale,
    SeatOccupied,
}

impl ImportCommitRejectionReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::PreviewTokenMismatch => "preview_token_mismatch",
            Self::CandidateChanged => "candidate_changed",
            Self::BaselineStale => "baseline_stale",
            Self::SeatOccupied => "seat_occupied",
        }
    }
}

pub(crate) struct ImportCommitAuditFacts {
    pub(crate) seats_added: usize,
    pub(crate) seats_removed: usize,
    pub(crate) mappings_changed: usize,
    pub(crate) binding_impacts: usize,
    pub(crate) credentials_advanced: usize,
}

#[derive(Debug)]
pub(crate) struct AuditEvent {
    pub(crate) id: AuditEventId,
    pub(crate) actor: &'static str,
    pub(crate) action_kind: &'static str,
    pub(crate) resource_type: &'static str,
    pub(crate) resource_id: Option<String>,
    pub(crate) result: &'static str,
    pub(crate) reason_code: Option<&'static str>,
    pub(crate) correlation_id: CorrelationId,
    pub(crate) group_correlation_id: Option<String>,
    pub(crate) detail: AuditDetail,
}

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    event: &AuditEvent,
) -> Result<(), AuditPersistenceError> {
    insert_on_connection(transaction.connection(), event)
}

fn insert_on_connection(
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
            audit_events::occurred_at_unix_ms
                .eq(sql::<BigInt>("CAST(unixepoch('subsec') * 1000 AS INTEGER)")),
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
