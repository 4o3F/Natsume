use uuid::Uuid;

use super::{
    AuditDetail, AuditEvent, AuditEventId, CorrelationId, ImportCommitAuditFacts,
    ImportCommitRejectionReason,
};

impl AuditEvent {
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
}
