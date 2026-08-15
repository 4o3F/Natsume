use diesel::{ExpressionMethods, RunQueryDsl, dsl::sql, sql_types::Text, sqlite::SqliteConnection};
use serde::Serialize;
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    application::{
        contest::DeviceLifecycleAction,
        enrollment::{EnrollmentResolution, IssuanceReason},
        provisioning::ProvisioningWindowAction,
    },
    db::schema::audit_events,
};

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
    },
    EnrollmentRequestApproved {},
    EnrollmentRequestRejected {},
    EnrollmentRequestsExpired {
        expired_count: i64,
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
        operator_id: String,
        removed_session_count: usize,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "system:password-reset",
            action_kind: "reset_operator_password",
            resource_type: "operator_account",
            resource_id: Some(operator_id),
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

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn import_candidate_created(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        candidate_id: Uuid,
        seats_added_count: usize,
        seats_removed_count: usize,
        mappings_changed_count: usize,
        binding_impact_count: usize,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "create_import_candidate",
            resource_type: "import_candidate",
            resource_id: Some(candidate_id.to_string()),
            result: "succeeded",
            reason_code: Some("operator_requested"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::ImportCandidateCreated {
                seats_added_count,
                seats_removed_count,
                mappings_changed_count,
                binding_impact_count,
            },
        }
    }

    #[must_use]
    pub(crate) fn import_candidate_rejected(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "create_import_candidate",
            resource_type: "import_candidate",
            resource_id: None,
            result: "rejected",
            reason_code: Some("candidate_invalid"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::ImportCandidateRejected {},
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn import_candidate_expired(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        candidate_id: Uuid,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "system:expiry",
            action_kind: "expire_import_candidate",
            resource_type: "import_candidate",
            resource_id: Some(candidate_id.to_string()),
            result: "succeeded",
            reason_code: Some("absolute_expiry_observed"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::ImportCandidateExpired {},
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn import_committed(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        candidate_id: Uuid,
        facts: &ImportCommitAuditFacts,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "commit_import",
            resource_type: "import_candidate",
            resource_id: Some(candidate_id.to_string()),
            result: "succeeded",
            reason_code: Some("operator_requested"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::ImportCommitted {
                seats_added_count: facts.seats_added_count,
                seats_removed_count: facts.seats_removed_count,
                mappings_changed_count: facts.mappings_changed_count,
                binding_impact_count: facts.binding_impact_count,
                credential_revision_advanced_count: facts.credential_revision_advanced_count,
                configuration_revision_advanced: facts.configuration_revision_advanced,
                binding_revision_advanced: facts.binding_revision_advanced,
            },
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn import_commit_rejected(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        candidate_id: Uuid,
        reason: ImportCommitRejectionReason,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "commit_import",
            resource_type: "import_candidate",
            resource_id: Some(candidate_id.to_string()),
            result: "rejected",
            reason_code: Some(reason.as_str()),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::ImportCommitRejected {},
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn import_candidate_discarded(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        candidate_id: Uuid,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "discard_import_candidate",
            resource_type: "import_candidate",
            resource_id: Some(candidate_id.to_string()),
            result: "succeeded",
            reason_code: Some("operator_requested"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::ImportCandidateDiscarded {},
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
        resolution: EnrollmentResolution,
        reason: IssuanceReason,
        certificate_serial: String,
        gateway_spki_sha256: [u8; 32],
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "device:enrollment",
            action_kind: "issue_device_credentials",
            resource_type: "enrollment_request",
            resource_id: Some(enrollment_request_id.to_string()),
            result: "succeeded",
            reason_code: Some(reason.as_audit_reason()),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::DeviceCredentialsIssued {
                resolution: resolution.as_persisted(),
                certificate_serial,
                gateway_spki_sha256: hex::encode(gateway_spki_sha256),
            },
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn enrollment_request_approved(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        enrollment_request_id: Uuid,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "approve_enrollment_request",
            resource_type: "enrollment_request",
            resource_id: Some(enrollment_request_id.to_string()),
            result: "succeeded",
            reason_code: Some("operator_requested"),
            correlation_id,
            group_correlation_id: None,
            detail: AuditDetail::EnrollmentRequestApproved {},
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn enrollment_request_rejected(
        audit_event_id: AuditEventId,
        correlation_id: CorrelationId,
        enrollment_request_id: Uuid,
    ) -> Self {
        Self {
            id: audit_event_id,
            actor: "operator:self",
            action_kind: "reject_enrollment_request",
            resource_type: "enrollment_request",
            resource_id: Some(enrollment_request_id.to_string()),
            result: "succeeded",
            reason_code: Some("operator_requested"),
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
