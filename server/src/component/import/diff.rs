use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{CandidateRowFacts, ImportError, baseline::BaselineSeat};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct ImportMappingChange {
    seat_code: String,
    current_domjudge_username: Option<String>,
    candidate_domjudge_username: String,
}

impl ImportMappingChange {
    fn new(
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
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct ImportBindingImpact {
    seat_code: String,
    device_id: String,
}

impl ImportBindingImpact {
    fn new(seat_code: String, device_id: String) -> Self {
        Self {
            seat_code,
            device_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct RedactedImportPreview {
    seats_added: Vec<String>,
    seats_removed: Vec<String>,
    mappings_changed: Vec<ImportMappingChange>,
    unchanged_count: usize,
    affected_account_count: usize,
    binding_impacts: Vec<ImportBindingImpact>,
}

impl RedactedImportPreview {
    fn new(
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
    pub(crate) fn mappings_changed(
        &self,
    ) -> impl ExactSizeIterator<Item = (&str, Option<&str>, &str)> {
        self.mappings_changed.iter().map(|change| {
            (
                change.seat_code.as_str(),
                change.current_domjudge_username.as_deref(),
                change.candidate_domjudge_username.as_str(),
            )
        })
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
    pub(crate) fn binding_impacts(&self) -> impl ExactSizeIterator<Item = (&str, &str)> {
        self.binding_impacts
            .iter()
            .map(|impact| (impact.seat_code.as_str(), impact.device_id.as_str()))
    }
}

pub(super) fn compute_diff(
    current_rows: &BTreeMap<String, BaselineSeat>,
    candidate_rows: &[CandidateRowFacts],
) -> Result<RedactedImportPreview, ImportError> {
    let mut candidate = BTreeMap::new();
    let mut candidate_accounts = BTreeSet::new();
    for row in candidate_rows {
        if candidate
            .insert(row.seat_code(), row.domjudge_username())
            .is_some()
            || !candidate_accounts.insert(row.domjudge_username())
        {
            return Err(ImportError::CandidateInvalid);
        }
    }
    if candidate.is_empty() {
        return Err(ImportError::CandidateInvalid);
    }

    let seats_added = candidate
        .keys()
        .filter(|seat_code| !current_rows.contains_key(**seat_code))
        .map(|seat_code| (*seat_code).to_owned())
        .collect();
    let mut seats_removed = Vec::new();
    let mut mappings_changed = Vec::new();
    let mut unchanged_count = 0;
    let mut binding_impacts = Vec::new();

    for (seat_code, facts) in current_rows {
        let Some(candidate_username) = candidate.get(seat_code.as_str()) else {
            seats_removed.push(seat_code.to_owned());
            if let Some(device_id) = facts.device_id() {
                binding_impacts.push(ImportBindingImpact::new(
                    seat_code.to_owned(),
                    device_id.as_text(),
                ));
            }
            continue;
        };
        if facts.current_domjudge_username() == Some(*candidate_username) {
            unchanged_count += 1;
        } else {
            mappings_changed.push(ImportMappingChange::new(
                seat_code.to_owned(),
                facts.current_domjudge_username().map(str::to_owned),
                (*candidate_username).to_owned(),
            ));
        }
    }

    Ok(RedactedImportPreview::new(
        seats_added,
        seats_removed,
        mappings_changed,
        unchanged_count,
        candidate_accounts.len(),
        binding_impacts,
    ))
}
