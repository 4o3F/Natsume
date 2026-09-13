mod baseline;
mod candidate;
mod commit;
mod db;
mod diff;
mod roster;

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::{db::Database, vault::VaultSession};

pub(crate) use self::candidate::{CreatedImportCandidate, ImportError, PendingImportCandidate};
pub(crate) use self::diff::RedactedImportPreview;
use self::roster::CandidateRoster;
pub(crate) use self::roster::{OrganizationDetails, TeamDetails};

/// Full-roster XLSX import authority with private persistence and a startup-loaded vault.
pub(crate) struct ImportComponent {
    database: Database,
    vault: Arc<VaultSession>,
}

impl ImportComponent {
    pub(crate) const fn new(database: Database, vault: Arc<VaultSession>) -> Self {
        Self { database, vault }
    }

    pub(crate) async fn create_candidate(
        &self,
        raw_xlsx: &[u8],
    ) -> Result<CreatedImportCandidate, ImportError> {
        candidate::create_import_candidate(&self.database, Arc::clone(&self.vault), raw_xlsx).await
    }

    pub(crate) async fn read_pending(&self) -> Result<Option<PendingImportCandidate>, ImportError> {
        candidate::read_pending_import_candidate(&self.database).await
    }

    pub(crate) async fn commit(
        &self,
        candidate_id: uuid::Uuid,
        presented_token: &[u8; 32],
        raw_xlsx: &[u8],
    ) -> Result<(), ImportError> {
        commit::commit_import(
            &self.database,
            Arc::clone(&self.vault),
            candidate_id,
            presented_token,
            raw_xlsx,
        )
        .await
    }

    pub(crate) async fn discard(&self, candidate_id: uuid::Uuid) -> Result<(), ImportError> {
        candidate::discard_import(&self.database, candidate_id).await
    }
}

const FINGERPRINT_VERSION: i32 = 2;

async fn parse_roster(raw: &[u8]) -> Result<natsume_roster::Roster, ImportError> {
    if raw.len() > natsume_roster::MAX_WORKBOOK_BYTES {
        return Err(ImportError::InvalidWorkbook(natsume_roster::InputError {
            row: None,
            column: None,
            kind: natsume_roster::InputErrorKind::WorkbookTooLarge,
        }));
    }
    let bytes = zeroize::Zeroizing::new(raw.to_vec());
    tokio::task::spawn_blocking(move || natsume_roster::parse_xlsx(&bytes))
        .await
        .map_err(|_| ImportError::CandidateInvalid)?
        .map_err(ImportError::InvalidWorkbook)
}

fn candidate_fingerprint(roster: &CandidateRoster) -> Result<[u8; 32], ImportError> {
    let bytes = serde_json::to_vec(roster).map_err(|_| ImportError::CandidateInvalid)?;
    let mut hasher = Sha256::new();
    write_field(&mut hasher, b"natsume/import-candidate/v2");
    write_field(&mut hasher, &bytes);
    Ok(hasher.finalize().into())
}

fn write_optional_field(hasher: &mut Sha256, value: Option<&[u8]>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            write_field(hasher, value);
        }
        None => hasher.update([0]),
    }
}

fn write_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests;
