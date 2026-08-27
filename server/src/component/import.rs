mod audit;
mod candidate;
mod commit;
mod csv;
mod db;
mod diff;

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::component::{
    contest::{CurrentAccountProjection, CurrentSeatProjection},
    lifecycle::DeviceId,
};

pub(crate) use self::candidate::{
    ImportError, PendingImportCandidate, PreviewToken, audit_invalid_import_upload,
    create_import_candidate, discard_import, read_pending_import_candidate,
};
pub(crate) use self::commit::commit_import;
pub(crate) use self::csv::CsvImportErrorCategory;
#[cfg(test)]
pub(crate) use self::csv::parse_csv;
pub(crate) use self::diff::{ImportBindingImpact, ImportMappingChange, RedactedImportPreview};
use self::{candidate::CandidateRowFacts, csv::ImportRow};

const FINGERPRINT_VERSION: i32 = 1;

fn candidate_fingerprint(rows: &[CandidateRowFacts]) -> [u8; 32] {
    let mut ordered = BTreeMap::new();
    for row in rows {
        ordered.insert(row.seat_code.as_str(), row.domjudge_username.as_str());
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
                .seal(row.password.as_bytes())
                .map_err(|_| ImportError::VaultFailure)?;
            Ok(candidate::SealedCommitRow {
                seat_code: row.seat_code.clone(),
                domjudge_username: row.domjudge_username.clone(),
                nonce,
                ciphertext,
            })
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
