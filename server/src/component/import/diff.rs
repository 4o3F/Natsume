use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    baseline::ImportBaseline,
    roster::{CandidateRoster, OrganizationDetails, TeamDetails},
};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct ImportMappingChange {
    pub(crate) seat_code: String,
    pub(crate) current_domjudge_username: Option<String>,
    pub(crate) candidate_domjudge_username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct ImportBindingImpact {
    pub(crate) seat_code: String,
    pub(crate) device_id: String,
    pub(crate) blocks_commit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct OrganizationChange {
    pub(crate) current: Option<OrganizationDetails>,
    pub(crate) candidate: Option<OrganizationDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct TeamChange {
    pub(crate) account: String,
    pub(crate) current: Option<TeamDetails>,
    pub(crate) candidate: Option<TeamDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RedactedImportPreview {
    pub(crate) seats_added: Vec<String>,
    pub(crate) seats_removed: Vec<String>,
    pub(crate) mappings_changed: Vec<ImportMappingChange>,
    pub(crate) unchanged_count: usize,
    pub(crate) affected_account_count: usize,
    pub(crate) binding_impacts: Vec<ImportBindingImpact>,
    pub(crate) accounts_added: Vec<String>,
    pub(crate) accounts_removed: Vec<String>,
    pub(crate) passwords_changed: Vec<String>,
    pub(crate) organizations: Vec<OrganizationDetails>,
    pub(crate) organization_changes: Vec<OrganizationChange>,
    pub(crate) team_changes: Vec<TeamChange>,
}

impl RedactedImportPreview {
    pub(super) fn blocks_commit(&self) -> bool {
        self.binding_impacts
            .iter()
            .any(|impact| impact.blocks_commit)
    }

    pub(super) fn has_changes(&self) -> bool {
        !self.seats_added.is_empty()
            || !self.seats_removed.is_empty()
            || !self.mappings_changed.is_empty()
            || !self.accounts_added.is_empty()
            || !self.accounts_removed.is_empty()
            || !self.passwords_changed.is_empty()
            || !self.organization_changes.is_empty()
            || !self.team_changes.is_empty()
    }
}

pub(super) fn compute_diff(
    baseline: &ImportBaseline,
    roster: &CandidateRoster,
    passwords_changed: Vec<String>,
) -> RedactedImportPreview {
    let seats = roster
        .teams
        .values()
        .filter_map(|team| team.seat.as_ref().map(|seat| (seat, &team.account)))
        .collect::<BTreeMap<_, _>>();
    let seats_added = seats
        .keys()
        .filter(|seat| !baseline.seats.contains_key(seat.as_str()))
        .map(|seat| (*seat).clone())
        .collect();
    let mut seats_removed = Vec::new();
    let mut mappings_changed = Vec::new();
    for (code, current) in &baseline.seats {
        match seats.get(code) {
            None => seats_removed.push(code.clone()),
            Some(account) if current.current_domjudge_username() != Some(account.as_str()) => {
                mappings_changed.push(ImportMappingChange {
                    seat_code: code.clone(),
                    current_domjudge_username: current
                        .current_domjudge_username()
                        .map(str::to_owned),
                    candidate_domjudge_username: (*account).clone(),
                });
            }
            Some(_) => {}
        }
    }
    let accounts_added = roster
        .teams
        .keys()
        .filter(|key| !baseline.accounts.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let accounts_removed = baseline
        .accounts
        .keys()
        .filter(|key| !roster.teams.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    let team_changes = team_changes(baseline, roster);
    let organization_changes = organization_changes(baseline, roster);
    let changed_school_ids = organization_changes
        .iter()
        .flat_map(|change| [change.current.as_ref(), change.candidate.as_ref()])
        .flatten()
        .map(|school| school.organization_id)
        .collect::<BTreeSet<_>>();
    let mut affected = accounts_added
        .iter()
        .chain(&accounts_removed)
        .chain(&passwords_changed)
        .cloned()
        .collect::<BTreeSet<_>>();
    affected.extend(team_changes.iter().map(|change| change.account.clone()));
    affected.extend(
        baseline
            .teams
            .values()
            .chain(roster.teams.values())
            .filter(|team| changed_school_ids.contains(&team.organization_id))
            .map(|team| team.account.clone()),
    );
    for change in &mappings_changed {
        affected.extend(change.current_domjudge_username.iter().cloned());
        affected.insert(change.candidate_domjudge_username.clone());
    }
    RedactedImportPreview {
        seats_added,
        seats_removed,
        mappings_changed,
        unchanged_count: roster
            .teams
            .keys()
            .filter(|account| !affected.contains(*account))
            .count(),
        affected_account_count: affected.len(),
        binding_impacts: binding_impacts(baseline, &seats, &affected),
        accounts_added,
        accounts_removed,
        passwords_changed,
        organizations: roster.organizations.values().cloned().collect(),
        organization_changes,
        team_changes,
    }
}

fn team_changes(baseline: &ImportBaseline, roster: &CandidateRoster) -> Vec<TeamChange> {
    let mut team_changes = Vec::new();
    for account in baseline
        .accounts
        .keys()
        .chain(roster.teams.keys())
        .collect::<BTreeSet<_>>()
    {
        let current = baseline.teams.get(account);
        let candidate = roster.teams.get(account);
        if current != candidate || candidate.is_none() {
            team_changes.push(TeamChange {
                account: account.clone(),
                current: current.cloned(),
                candidate: candidate.cloned(),
            });
        }
    }
    team_changes
}

fn organization_changes(
    baseline: &ImportBaseline,
    roster: &CandidateRoster,
) -> Vec<OrganizationChange> {
    let mut organization_changes = Vec::new();
    for key in baseline
        .organizations
        .keys()
        .chain(roster.organizations.keys())
        .collect::<BTreeSet<_>>()
    {
        let current = baseline.organizations.get(key);
        let candidate = roster.organizations.get(key);
        if current != candidate {
            organization_changes.push(OrganizationChange {
                current: current.cloned(),
                candidate: candidate.cloned(),
            });
        }
    }
    organization_changes
}

fn binding_impacts(
    baseline: &ImportBaseline,
    seats: &BTreeMap<&String, &String>,
    affected: &BTreeSet<String>,
) -> Vec<ImportBindingImpact> {
    let mut binding_impacts = Vec::new();
    for (code, current) in &baseline.seats {
        let Some(device) = current.device_id() else {
            continue;
        };
        let next = seats.get(code);
        if next.is_none()
            || current.current_domjudge_username() != next.map(|account| account.as_str())
            || current
                .current_domjudge_username()
                .is_some_and(|account| affected.contains(account))
            || next.is_some_and(|account| affected.contains(*account))
        {
            binding_impacts.push(ImportBindingImpact {
                seat_code: code.clone(),
                device_id: device.as_text(),
                blocks_commit: next.is_none(),
            });
        }
    }
    binding_impacts
}
