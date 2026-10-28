use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use snafu::ensure;

use super::{InvalidSnafu, Result};
use crate::roster::Team;

pub(super) const MANIFEST_LIMIT: usize = 4 * 1024 * 1024;
pub(super) const IMAGE_LIMIT: usize = 16 * 1024 * 1024;

pub(super) fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub(super) fn id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}
pub(super) fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

// Contains a freshly issued secret; never derive Debug.
#[derive(Deserialize, Serialize, zeroize::ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub(super) struct PairingCredentials {
    pub source_id: String,
    pub enrollment_id: String,
    pub token: String,
    pub expires_at_unix_ms: i64,
}
impl PairingCredentials {
    pub fn validate(&self) -> Result<()> {
        let prefix = format!("nph1.{}.{}.", self.source_id, self.enrollment_id);
        ensure!(
            id(&self.source_id)
                && id(&self.enrollment_id)
                && self.expires_at_unix_ms > 0
                && self.token.strip_prefix(&prefix).is_some_and(hash),
            InvalidSnafu
        );
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub format_version: u32,
    pub source_id: String,
    pub dataset_id: String,
    pub initialization_id: String,
    pub teams: Vec<Team>,
    pub school_logos: Vec<Logo>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Logo {
    pub organization_id: i64,
    pub sha256: String,
    pub content_type: String,
    pub size_bytes: u64,
}
impl Manifest {
    pub fn validate(&self, source: &str) -> Result<()> {
        ensure!(
            self.format_version == 1
                && self.source_id == source
                && id(&self.dataset_id)
                && id(&self.initialization_id)
                && !self.teams.is_empty()
                && self.teams.len() <= 2000,
            InvalidSnafu
        );
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
                InvalidSnafu
            );
            if let Some(previous) = images.insert(&logo.sha256, logo.size_bytes) {
                ensure!(previous == logo.size_bytes, InvalidSnafu);
            }
        }
        ensure!(
            images.values().sum::<u64>() <= 256 * 1024 * 1024,
            InvalidSnafu
        );
        Ok(())
    }
}
