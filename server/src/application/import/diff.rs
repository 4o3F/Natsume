use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct ImportMappingChange {
    pub(super) seat_code: String,
    pub(super) current_domjudge_username: Option<String>,
    pub(super) candidate_domjudge_username: String,
}

impl ImportMappingChange {
    pub(crate) fn new(
        seat_code: String,
        current_domjudge_username: Option<String>,
        candidate_domjudge_username: String,
    ) -> Self {
        Self {
            seat_code,
            current_domjudge_username,
            candidate_domjudge_username,
        }
    }

    #[must_use]
    pub(crate) fn seat_code(&self) -> &str {
        &self.seat_code
    }

    #[must_use]
    pub(crate) fn current_domjudge_username(&self) -> Option<&str> {
        self.current_domjudge_username.as_deref()
    }

    #[must_use]
    pub(crate) fn candidate_domjudge_username(&self) -> &str {
        &self.candidate_domjudge_username
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct ImportBindingImpact {
    pub(super) seat_code: String,
    pub(super) device_id: String,
}

impl ImportBindingImpact {
    pub(crate) fn new(seat_code: String, device_id: String) -> Self {
        Self {
            seat_code,
            device_id,
        }
    }

    #[must_use]
    pub(crate) fn seat_code(&self) -> &str {
        &self.seat_code
    }

    #[must_use]
    pub(crate) fn device_id(&self) -> &str {
        &self.device_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct RedactedImportPreview {
    pub(super) seats_added: Vec<String>,
    pub(super) seats_removed: Vec<String>,
    pub(super) mappings_changed: Vec<ImportMappingChange>,
    pub(super) unchanged_count: usize,
    pub(super) affected_account_count: usize,
    pub(super) binding_impacts: Vec<ImportBindingImpact>,
}

impl RedactedImportPreview {
    pub(crate) fn new(
        seats_added: Vec<String>,
        seats_removed: Vec<String>,
        mappings_changed: Vec<ImportMappingChange>,
        unchanged_count: usize,
        affected_account_count: usize,
        binding_impacts: Vec<ImportBindingImpact>,
    ) -> Self {
        Self {
            seats_added,
            seats_removed,
            mappings_changed,
            unchanged_count,
            affected_account_count,
            binding_impacts,
        }
    }

    #[must_use]
    pub(crate) fn seats_added(&self) -> &[String] {
        &self.seats_added
    }

    #[must_use]
    pub(crate) fn seats_removed(&self) -> &[String] {
        &self.seats_removed
    }

    #[must_use]
    pub(crate) fn mappings_changed(&self) -> &[ImportMappingChange] {
        &self.mappings_changed
    }

    #[must_use]
    pub(crate) const fn unchanged_count(&self) -> usize {
        self.unchanged_count
    }

    #[must_use]
    pub(crate) const fn affected_account_count(&self) -> usize {
        self.affected_account_count
    }

    #[must_use]
    pub(crate) fn binding_impacts(&self) -> &[ImportBindingImpact] {
        &self.binding_impacts
    }
}
