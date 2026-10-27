mod baseline;
mod candidate;
mod commit;
mod db;
mod diff;
mod roster;
mod workbook;

use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{
    db::{Database, Transaction, TransactionError},
    device_control::TargetChange,
    vault::VaultSession,
};

pub(crate) use self::candidate::{CreatedImportCandidate, ImportError, PendingImportCandidate};
pub(crate) use self::diff::RedactedImportPreview;
use self::roster::CandidateRoster;
pub(crate) use self::roster::{OrganizationDetails, TeamDetails};
pub(crate) use self::workbook::{InputError, InputErrorKind, MAX_WORKBOOK_BYTES};

pub(crate) const XLSX_TEMPLATE: &[u8] = include_bytes!("import/examples/template.xlsx");

/// Full-roster XLSX import authority with private persistence and a startup-loaded vault.
pub(crate) struct ImportComponent {
    database: Database,
    vault: Arc<VaultSession>,
    work: Arc<Semaphore>,
}

/// One admitted import, from request-body reading through blocking work.
/// Consuming methods prevent starting two operations with the same permit.
pub(crate) struct ImportOperation {
    database: Database,
    vault: Arc<VaultSession>,
    permit: OwnedSemaphorePermit,
}

impl ImportComponent {
    pub(crate) fn new(database: Database, vault: Arc<VaultSession>) -> Self {
        Self {
            database,
            vault,
            work: Arc::new(Semaphore::new(1)),
        }
    }

    pub(crate) fn try_begin(&self) -> Result<ImportOperation, ImportError> {
        let permit = Arc::clone(&self.work)
            .try_acquire_owned()
            // Capacity exhaustion is a domain rejection; semaphore detail does not change the decision.
            .map_err(|_| ImportError::Busy)?;
        Ok(ImportOperation {
            database: self.database.clone(),
            vault: Arc::clone(&self.vault),
            permit,
        })
    }

    pub(crate) async fn read_pending(&self) -> Result<Option<PendingImportCandidate>, ImportError> {
        candidate::read_pending_import_candidate(&self.database).await
    }

    pub(crate) async fn discard(&self, candidate_id: uuid::Uuid) -> Result<(), ImportError> {
        candidate::discard_import(&self.database, candidate_id).await
    }
}

impl ImportOperation {
    pub(crate) async fn create_candidate(
        self,
        raw_xlsx: &[u8],
    ) -> Result<CreatedImportCandidate, ImportError> {
        candidate::create_import_candidate(&self.database, self.vault, raw_xlsx, self.permit).await
    }

    pub(crate) async fn commit(
        self,
        candidate_id: uuid::Uuid,
        presented_token: &[u8; 32],
        raw_xlsx: &[u8],
    ) -> Result<TargetChange, ImportError> {
        commit::commit_import(
            &self.database,
            self.vault,
            candidate_id,
            presented_token,
            raw_xlsx,
            self.permit,
        )
        .await
        .map(|()| TargetChange::all_connected())
    }
}

const FINGERPRINT_VERSION: i32 = 2;

async fn parse_roster(
    raw: &[u8],
    permit: OwnedSemaphorePermit,
) -> Result<(workbook::Roster, OwnedSemaphorePermit), ImportError> {
    if raw.len() > MAX_WORKBOOK_BYTES {
        return Err(ImportError::InvalidWorkbook {
            source: InputError {
                row: None,
                column: None,
                kind: InputErrorKind::WorkbookTooLarge,
            },
        });
    }
    let bytes = zeroize::Zeroizing::new(raw.to_vec());
    // Return the permit with the result. Dropping the caller cannot release it
    // while the blocking closure is still queued or parsing.
    tokio::task::spawn_blocking(move || workbook::parse_xlsx(&bytes).map(|roster| (roster, permit)))
        .await
        .map_err(|source| ImportError::CandidateWorker {
            source: source.into(),
        })?
        .map_err(|source| ImportError::InvalidWorkbook { source })
}

// Carry capacity in both transaction outcomes until COMMIT/ROLLBACK completes,
// including when the async caller has already been cancelled.
async fn write_import<T: Send + 'static>(
    database: &Database,
    permit: OwnedSemaphorePermit,
    operation: impl FnOnce(&mut Transaction<'_>) -> Result<T, ImportError> + Send + 'static,
) -> Result<T, ImportError> {
    database
        .write(move |transaction| match operation(transaction) {
            Ok(value) => Ok((value, permit)),
            Err(error) => Err((error, permit)),
        })
        .await
        .map(|(value, _permit)| value)
        .map_err(|error| match error {
            TransactionError::Operation((error, _permit)) => error,
            TransactionError::Persistence(error) => error.into(),
        })
}

fn candidate_fingerprint(roster: &CandidateRoster) -> Result<[u8; 32], ImportError> {
    let bytes =
        serde_json::to_vec(roster).map_err(|source| ImportError::CandidateEncoding { source })?;
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
pub(crate) mod tests;
