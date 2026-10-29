//! Platform-independent Photo initialization contract and validation.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use snafu::{Snafu, ensure};

pub const MANIFEST_LIMIT: usize = 4 * 1024 * 1024;
pub const IMAGE_LIMIT: usize = 16 * 1024 * 1024;
pub const RESOURCE_LIMIT: u64 = 256 * 1024 * 1024;
pub const TEAM_LIMIT: usize = 2000;

#[derive(Debug, Snafu)]
#[snafu(display("invalid Photo contract: {field}"))]
pub struct ValidationError {
    pub field: &'static str,
}

#[must_use]
pub fn id(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|id| id.get_version_num() == 7 && id.to_string() == value)
}

#[must_use]
pub fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Parses only the credential shape; Server still authenticates its stored hash.
#[must_use]
pub fn credential_parts(token: &str) -> Option<(&str, &str)> {
    let mut parts = token.split('.');
    let (Some("nph1"), Some(source), Some(enrollment), Some(secret), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    (id(source) && id(enrollment) && hash(secret)).then_some((source, enrollment))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct PhotoTeam {
    pub account_id: String,
    /// External Team ID, the `DOMjudge` username; never a password or seat.
    pub account: String,
    #[cfg_attr(feature = "typescript", specta(type = specta_typescript::Number))]
    pub organization_id: i64,
    pub school_name_zh: String,
    #[serde(default)]
    pub school_name_en: String,
    pub team_name_zh: String,
    #[serde(default)]
    pub team_name_en: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct PhotoLogo {
    #[cfg_attr(feature = "typescript", specta(type = specta_typescript::Number))]
    pub organization_id: i64,
    pub sha256: String,
    pub content_type: String,
    #[cfg_attr(feature = "typescript", specta(type = specta_typescript::Number))]
    pub size_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct PhotoInitialization {
    pub format_version: u32,
    pub source_id: String,
    pub dataset_id: String,
    pub initialization_id: String,
    pub teams: Vec<PhotoTeam>,
    pub school_logos: Vec<PhotoLogo>,
}

impl PhotoInitialization {
    /// Validates a snapshot before publication and after download.
    /// # Errors
    /// Rejects invalid identities, relationships, duplicates and resource limits.
    pub fn validate(&self, source: &str) -> Result<(), ValidationError> {
        ensure!(
            self.format_version == 1
                && id(source)
                && self.source_id == source
                && id(&self.dataset_id)
                && id(&self.initialization_id),
            ValidationSnafu {
                field: "snapshot identity"
            }
        );
        validate_teams(&self.teams)?;
        let organizations: BTreeSet<_> = self.teams.iter().map(|t| t.organization_id).collect();
        let mut schools = BTreeSet::new();
        let mut images = BTreeMap::new();
        for logo in &self.school_logos {
            ensure!(
                organizations.contains(&logo.organization_id)
                    && schools.insert(logo.organization_id)
                    && hash(&logo.sha256)
                    && logo.content_type == "image/png"
                    && (1..=IMAGE_LIMIT as u64).contains(&logo.size_bytes),
                ValidationSnafu {
                    field: "school logo"
                }
            );
            if let Some(previous) = images.insert(&logo.sha256, logo.size_bytes) {
                ensure!(
                    previous == logo.size_bytes,
                    ValidationSnafu { field: "logo size" }
                );
            }
        }
        ensure!(
            images.values().sum::<u64>() <= RESOURCE_LIMIT,
            ValidationSnafu {
                field: "resource limit"
            }
        );
        Ok(())
    }
}

/// Checks the same roster invariants at both ends of initialization.
/// # Errors
/// Rejects empty/oversized rosters or empty/duplicate account identities.
pub fn validate_teams(teams: &[PhotoTeam]) -> Result<(), ValidationError> {
    ensure!(
        !teams.is_empty() && teams.len() <= TEAM_LIMIT,
        ValidationSnafu {
            field: "team count"
        }
    );
    let mut ids = BTreeSet::new();
    let mut accounts = BTreeSet::new();
    for team in teams {
        ensure!(
            !team.account_id.trim().is_empty()
                && !team.account.trim().is_empty()
                && ids.insert(&team.account_id)
                && accounts.insert(&team.account),
            ValidationSnafu {
                field: "team identity"
            }
        );
    }
    Ok(())
}

/// Contains a secret: intentionally has no Debug implementation.
#[derive(Serialize, Deserialize, zeroize::ZeroizeOnDrop)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct PhotoEnrollmentIssued {
    pub source_id: String,
    pub enrollment_id: String,
    pub token: String,
    #[cfg_attr(feature = "typescript", specta(type = specta_typescript::Number))]
    pub expires_at_unix_ms: i64,
}

impl PhotoEnrollmentIssued {
    /// # Errors
    /// Rejects noncanonical identifiers or malformed credentials without exposing them.
    pub fn validate(&self) -> Result<(), ValidationError> {
        ensure!(
            self.expires_at_unix_ms > 0
                && credential_parts(&self.token)
                    == Some((self.source_id.as_str(), self.enrollment_id.as_str())),
            ValidationSnafu {
                field: "enrollment credential"
            }
        );
        Ok(())
    }
}
