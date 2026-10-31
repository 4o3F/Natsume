//! Platform-independent Photo initialization and offline result contracts.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use snafu::{Snafu, ensure};

pub const MANIFEST_LIMIT: usize = 4 * 1024 * 1024;
pub const IMAGE_LIMIT: usize = 16 * 1024 * 1024;
pub const RESOURCE_LIMIT: u64 = 256 * 1024 * 1024;
pub const TEAM_LIMIT: usize = 2000;
/// Maximum encoded pair/confirm response size; excludes snapshots and images.
pub const ENROLLMENT_RESPONSE_LIMIT: usize = 8192;

pub mod results;
mod value;
pub use value::{
    DatasetId, EnrollmentId, Identity, InitializationId, InitializationRequestId, Sha256Digest,
    SigningPublicKey, SourceId, ValueError, is_canonical_uuid_v7, is_lowercase_sha256,
};

/// Relationship or capacity failure in an otherwise decoded snapshot.
#[derive(Debug, Snafu)]
pub enum ValidationError {
    #[snafu(display("snapshot source does not match the enrollment"))]
    Source,
    #[snafu(display("roster must contain between one and 2000 teams"))]
    TeamCount,
    #[snafu(display("roster contains an empty or duplicate account"))]
    TeamIdentity,
    #[snafu(display("school logo reference or size is invalid"))]
    SchoolLogo,
    #[snafu(display("shared school logo has inconsistent sizes"))]
    LogoSize,
    #[snafu(display("snapshot exceeds the resource byte budget"))]
    ResourceLimit,
    #[snafu(display("issued enrollment credential is empty or expired"))]
    EnrollmentCredential,
}

/// The initialization snapshot format currently supported by both consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(try_from = "u32", into = "u32")]
#[cfg_attr(feature = "typescript", specta(type = u32))]
pub enum FormatVersion {
    V1,
}
impl TryFrom<u32> for FormatVersion {
    type Error = FormatVersionError;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        ensure!(value == 1, FormatVersionSnafu);
        Ok(Self::V1)
    }
}
impl From<FormatVersion> for u32 {
    fn from(value: FormatVersion) -> u32 {
        match value {
            FormatVersion::V1 => 1,
        }
    }
}
/// An unsupported snapshot version; never silently interpreted as the current one.
#[derive(Debug, Snafu)]
#[snafu(display("unsupported Photo snapshot version"))]
pub struct FormatVersionError;

/// Logos are normalized by Contest before entering this protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
pub enum LogoContentType {
    #[serde(rename = "image/png")]
    Png,
}
impl LogoContentType {
    /// HTTP media type for this normalized format.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
        }
    }
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
    pub sha256: Sha256Digest,
    pub content_type: LogoContentType,
    #[cfg_attr(feature = "typescript", specta(type = specta_typescript::Number))]
    pub size_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct PhotoInitialization {
    pub format_version: FormatVersion,
    pub source_id: SourceId,
    pub dataset_id: DatasetId,
    pub initialization_id: InitializationId,
    pub teams: Vec<PhotoTeam>,
    pub school_logos: Vec<PhotoLogo>,
}

impl PhotoInitialization {
    /// Validates a snapshot before publication and after download.
    /// # Errors
    /// Rejects invalid identities, relationships, duplicates and resource limits.
    pub fn validate(&self, source: &SourceId) -> Result<(), ValidationError> {
        ensure!(&self.source_id == source, SourceSnafu);
        validate_teams(&self.teams)?;
        let organizations: BTreeSet<_> = self.teams.iter().map(|t| t.organization_id).collect();
        let mut schools = BTreeSet::new();
        let mut images = BTreeMap::new();
        for logo in &self.school_logos {
            ensure!(
                organizations.contains(&logo.organization_id)
                    && schools.insert(logo.organization_id)
                    && (1..=IMAGE_LIMIT as u64).contains(&logo.size_bytes),
                SchoolLogoSnafu
            );
            if let Some(previous) = images.insert(&logo.sha256, logo.size_bytes) {
                ensure!(previous == logo.size_bytes, LogoSizeSnafu);
            }
        }
        ensure!(
            images.values().sum::<u64>() <= RESOURCE_LIMIT,
            ResourceLimitSnafu
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
        TeamCountSnafu
    );
    let mut ids = BTreeSet::new();
    let mut accounts = BTreeSet::new();
    for team in teams {
        ensure!(
            !team.account_id.trim().is_empty()
                && !team.account.trim().is_empty()
                && ids.insert(&team.account_id)
                && accounts.insert(&team.account),
            TeamIdentitySnafu
        );
    }
    Ok(())
}

/// Contains a secret: intentionally has no Debug implementation.
#[derive(Serialize, Deserialize, zeroize::ZeroizeOnDrop)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct PhotoEnrollmentIssued {
    #[zeroize(skip)]
    pub source_id: SourceId,
    #[zeroize(skip)]
    pub enrollment_id: EnrollmentId,
    pub token: String,
    #[cfg_attr(feature = "typescript", specta(type = specta_typescript::Number))]
    pub expires_at_unix_ms: i64,
}

impl PhotoEnrollmentIssued {
    /// # Errors
    /// Rejects an empty token or invalid expiry without interpreting Server's token format.
    pub fn validate(&self) -> Result<(), ValidationError> {
        ensure!(
            self.expires_at_unix_ms > 0 && !self.token.is_empty(),
            EnrollmentCredentialSnafu
        );
        Ok(())
    }
}

/// Public source projection used by Operators and the enrollment confirmation.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct PhotoSource {
    pub source_id: SourceId,
    pub name: String,
    pub state: SourceState,
    pub enrollment_confirmed: bool,
    pub initialization_completed: bool,
}
/// Source lifecycle; authority remains entirely on Server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    Enabled,
    Disabled,
    Revoked,
}

/// First enrollment request. The one-time code is sensitive.
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct PhotoPairRequest {
    pub code: String,
    pub signing_public_key: SigningPublicKey,
    pub client_version: String,
}
/// Confirms that the issued enrollment is bound to the original signing key.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct PhotoConfirmation {
    pub enrollment_id: EnrollmentId,
    pub signing_public_key: SigningPublicKey,
}
/// Idempotent completion receipt, sent only after local integrity verification.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct PhotoInitializationComplete {
    pub initialization_id: InitializationId,
    pub manifest_sha256: Sha256Digest,
    pub request_id: InitializationRequestId,
}
