use std::collections::BTreeMap;

use natsume_roster::Roster;
use serde::{Deserialize, Serialize};

use crate::vault::VaultSession;

use super::{ImportError, baseline::ImportBaseline};

pub(crate) use crate::component::contest::OrganizationDetails;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TeamDetails {
    pub(crate) account: String,
    pub(crate) seat: Option<String>,
    pub(crate) organization_id: i64,
    pub(crate) name_zh: String,
    pub(crate) name_en: String,
    pub(crate) category: String,
}

/// Planned IDs are derived from the same guarded baseline as the preview.
/// Passwords and their digests never enter this value or its serialized form.
#[derive(Serialize)]
pub(super) struct CandidateRoster {
    pub(super) organizations: BTreeMap<String, OrganizationDetails>,
    pub(super) teams: BTreeMap<String, TeamDetails>,
}

impl CandidateRoster {
    pub(super) fn prepare(baseline: &ImportBaseline, roster: &Roster) -> Result<Self, ImportError> {
        let mut sequence = baseline.organization_sequence;
        let mut organizations = BTreeMap::new();
        for school in roster.organizations() {
            let organization_id = if let Some(current) = baseline.organizations.get(school.key()) {
                current.organization_id
            } else {
                sequence = sequence
                    .checked_add(1)
                    .ok_or(ImportError::PersistenceFailure)?;
                sequence
            };
            organizations.insert(
                school.key().to_owned(),
                OrganizationDetails {
                    organization_id,
                    name_zh: school.name_zh().to_owned(),
                    name_en: school.name_en().to_owned(),
                    country: school.country().to_owned(),
                },
            );
        }
        let mut teams = BTreeMap::new();
        for team in roster.teams() {
            let organization = organizations
                .get(team.organization_key())
                .ok_or(ImportError::CandidateInvalid)?;
            teams.insert(
                team.account().to_owned(),
                TeamDetails {
                    account: team.account().to_owned(),
                    seat: Some(team.seat().to_owned()),
                    organization_id: organization.organization_id,
                    name_zh: team.name_zh().to_owned(),
                    name_en: team.name_en().to_owned(),
                    category: team.category().to_owned(),
                },
            );
        }
        Ok(Self {
            organizations,
            teams,
        })
    }
}

pub(super) fn changed_passwords(
    baseline: &ImportBaseline,
    roster: &Roster,
    vault: &VaultSession,
) -> Result<Vec<String>, ImportError> {
    let mut changed = Vec::new();
    for team in roster.teams() {
        let Some(current) = baseline.accounts.get(team.account()) else {
            continue;
        };
        let plaintext = vault
            .open(&current.nonce, &current.ciphertext)
            .map_err(|_| ImportError::VaultFailure)?;
        if plaintext.as_slice() != team.password().as_bytes() {
            changed.push(team.account().to_owned());
        }
    }
    changed.sort();
    Ok(changed)
}
