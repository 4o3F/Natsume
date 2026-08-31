mod audit;
mod candidate;
mod commit;
mod csv;
mod db;
mod diff;

use std::{collections::BTreeMap, sync::Arc};

use sha2::{Digest, Sha256};

use crate::component::{
    contest::{CurrentAccountProjection, CurrentSeatProjection},
    lifecycle::DeviceId,
};
use crate::{audit::CorrelationId, db::Database, vault::VaultSession};

#[cfg(test)]
use self::candidate::create_import_candidate;
pub(crate) use self::candidate::{CreatedImportCandidate, ImportError, PendingImportCandidate};
#[cfg(test)]
use self::commit::commit_import;
pub(crate) use self::csv::CsvImportErrorCategory;
pub(crate) use self::diff::RedactedImportPreview;
use self::{candidate::CandidateRowFacts, csv::ImportRow};

/// CSV import authority with private persistence and a startup-loaded vault.
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
        raw_csv: &[u8],
        correlation_id: CorrelationId,
    ) -> Result<CreatedImportCandidate, ImportError> {
        match candidate::create_import_candidate(&self.database, raw_csv, correlation_id).await {
            Ok(candidate) => Ok(candidate),
            Err(error @ (ImportError::InvalidCsv(_) | ImportError::CandidateInvalid)) => {
                candidate::audit_invalid_import_upload(&self.database, correlation_id).await?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn read_pending(
        &self,
        correlation_id: CorrelationId,
    ) -> Result<Option<PendingImportCandidate>, ImportError> {
        candidate::read_pending_import_candidate(&self.database, correlation_id).await
    }

    pub(crate) async fn commit(
        &self,
        candidate_id: uuid::Uuid,
        presented_token: &[u8; 32],
        raw_csv: &[u8],
        correlation_id: CorrelationId,
    ) -> Result<(), ImportError> {
        commit::commit_import(
            &self.database,
            &self.vault,
            candidate_id,
            presented_token,
            raw_csv,
            correlation_id,
        )
        .await
    }

    pub(crate) async fn discard(
        &self,
        candidate_id: uuid::Uuid,
        correlation_id: CorrelationId,
    ) -> Result<(), ImportError> {
        candidate::discard_import(&self.database, candidate_id, correlation_id).await
    }
}

const FINGERPRINT_VERSION: i32 = 1;

fn candidate_fingerprint(rows: &[CandidateRowFacts]) -> [u8; 32] {
    let mut ordered = BTreeMap::new();
    for row in rows {
        ordered.insert(row.seat_code(), row.domjudge_username());
    }
    let mut hasher = Sha256::new();
    write_field(&mut hasher, b"natsume/import-candidate/v1");
    for (seat_code, username) in ordered {
        write_field(&mut hasher, seat_code.as_bytes());
        write_field(&mut hasher, username.as_bytes());
    }
    hasher.finalize().into()
}

fn current_fingerprint(
    seats: &[CurrentSeatProjection],
    accounts: &[CurrentAccountProjection],
) -> [u8; 32] {
    let mut ordered_seats = BTreeMap::new();
    for seat in seats {
        ordered_seats.insert(seat.seat_code(), seat);
    }
    let mut ordered_accounts = BTreeMap::new();
    for account in accounts {
        ordered_accounts.insert(account.domjudge_username(), account);
    }

    let mut hasher = Sha256::new();
    write_field(&mut hasher, b"natsume/import-baseline/v1");
    for seat in ordered_seats.into_values() {
        write_field(&mut hasher, b"seat");
        write_field(&mut hasher, seat.seat_id().as_bytes());
        write_field(&mut hasher, seat.seat_code().as_bytes());
        write_optional_field(
            &mut hasher,
            seat.current_domjudge_username().map(str::as_bytes),
        );
        let device_id = seat.device_id().map(DeviceId::as_text);
        write_optional_field(&mut hasher, device_id.as_deref().map(str::as_bytes));
    }
    for account in ordered_accounts.into_values() {
        write_field(&mut hasher, b"account");
        write_field(&mut hasher, account.account_id().as_bytes());
        write_field(&mut hasher, account.domjudge_username().as_bytes());
        write_field(&mut hasher, &account.credential_revision().to_be_bytes());
    }
    hasher.finalize().into()
}

fn seal_rows(
    vault: &crate::vault::VaultSession,
    rows: &[ImportRow],
) -> Result<Vec<candidate::SealedCommitRow>, ImportError> {
    rows.iter()
        .map(|row| {
            let (nonce, ciphertext) = vault
                .seal(row.password().as_bytes())
                .map_err(|_| ImportError::VaultFailure)?;
            Ok(candidate::SealedCommitRow::new(
                row.seat_code().to_owned(),
                row.domjudge_username().to_owned(),
                nonce,
                ciphertext,
            ))
        })
        .collect()
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
