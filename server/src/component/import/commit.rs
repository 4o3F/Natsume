use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    db::{Database, Transaction, TransactionError},
    vault::VaultSession,
};

use super::{
    FINGERPRINT_VERSION,
    baseline::{BaselineAccount, BaselineSeat, ImportBaseline},
    candidate::{CandidateExpiry, ImportError, SealedCommitRow, expire_candidate},
    candidate_fingerprint,
    csv::parse_csv,
    diff::{RedactedImportPreview, compute_diff},
    seal_rows,
};

pub(super) async fn commit_import(
    database: &Database,
    vault: &VaultSession,
    candidate_id: Uuid,
    presented_token: &[u8; 32],
    raw_csv: &[u8],
) -> Result<(), ImportError> {
    let parsed = parse_csv(raw_csv).map_err(|error| ImportError::InvalidCsv(error.category()))?;
    let candidate_rows = parsed.candidate_rows();
    let candidate_hash = candidate_fingerprint(&candidate_rows);
    let sealed_rows = seal_rows(vault, parsed.rows())?;
    drop(parsed);

    let token_hash = Sha256::digest(presented_token).into();
    let outcome = database
        .write(move |transaction| {
            commit_in_transaction(
                transaction,
                candidate_id,
                token_hash,
                candidate_hash,
                &candidate_rows,
                &sealed_rows,
            )
        })
        .await
        .map_err(TransactionError::into_error)?;

    match outcome {
        CommitOutcome::Committed => Ok(()),
        CommitOutcome::Unavailable => Err(ImportError::CandidateUnavailable),
        CommitOutcome::Stale => Err(ImportError::PreviewStale),
        CommitOutcome::SeatOccupied => Err(ImportError::SeatOccupied),
    }
}

fn commit_in_transaction(
    transaction: &mut Transaction<'_>,
    candidate_id: Uuid,
    token_hash: [u8; 32],
    candidate_hash: [u8; 32],
    candidate_rows: &[super::candidate::CandidateRowFacts],
    sealed_rows: &[SealedCommitRow],
) -> Result<CommitOutcome, ImportError> {
    let Some(candidate) = super::db::pending_import_candidate::find(transaction)? else {
        return Ok(CommitOutcome::Unavailable);
    };
    if candidate.candidate_id() != candidate_id {
        return Ok(CommitOutcome::Unavailable);
    }
    if candidate.expiry() == CandidateExpiry::Expired {
        expire_candidate(transaction, &candidate)?;
        return Ok(CommitOutcome::Unavailable);
    }
    if !bool::from(
        token_hash
            .as_slice()
            .ct_eq(candidate.preview_token_hash().as_slice()),
    ) {
        return Ok(CommitOutcome::Unavailable);
    }
    if candidate.fingerprint_version() != FINGERPRINT_VERSION
        || candidate.candidate_fingerprint_sha256() != &candidate_hash
    {
        return Ok(CommitOutcome::Stale);
    }

    let baseline = super::db::query::read_baseline(transaction)?;
    let baseline_hash = baseline.fingerprint();
    if candidate.baseline_fingerprint_sha256() != &baseline_hash {
        return Ok(CommitOutcome::Stale);
    }

    let plan = CommitPlan::prepare(baseline, candidate_rows)?;
    if plan.diff.binding_impacts().len() != 0 {
        return Ok(CommitOutcome::SeatOccupied);
    }
    apply_plan(transaction, sealed_rows, &plan)?;
    if super::db::pending_import_candidate::delete_exact(transaction, &candidate)? != 1 {
        return Err(ImportError::PersistenceFailure);
    }
    Ok(CommitOutcome::Committed)
}

struct CommitPlan {
    current_seats: BTreeMap<String, BaselineSeat>,
    current_accounts: BTreeMap<String, BaselineAccount>,
    candidate_usernames: BTreeSet<String>,
    next_credential_revisions: BTreeMap<String, i64>,
    diff: RedactedImportPreview,
}

impl CommitPlan {
    fn prepare(
        baseline: ImportBaseline,
        candidate_rows: &[super::candidate::CandidateRowFacts],
    ) -> Result<Self, ImportError> {
        let diff = compute_diff(baseline.seats(), candidate_rows)?;
        let (current_seats, current_accounts) = baseline.into_parts();
        let candidate_usernames = candidate_rows
            .iter()
            .map(|row| row.domjudge_username().to_owned())
            .collect::<BTreeSet<_>>();
        let mut next_credential_revisions = BTreeMap::new();
        for username in &candidate_usernames {
            if let Some(account) = current_accounts.get(username) {
                let next = account
                    .credential_revision()
                    .checked_add(1)
                    .ok_or(ImportError::PersistenceFailure)?;
                next_credential_revisions.insert(username.clone(), next);
            }
        }
        Ok(Self {
            current_seats,
            current_accounts,
            candidate_usernames,
            next_credential_revisions,
            diff,
        })
    }
}

fn apply_plan(
    transaction: &mut Transaction<'_>,
    sealed_rows: &[SealedCommitRow],
    plan: &CommitPlan,
) -> Result<(), ImportError> {
    let expected_mappings = plan
        .current_seats
        .values()
        .filter(|seat| seat.current_domjudge_username().is_some())
        .count();
    if super::db::account_mappings::delete_all(transaction)? != expected_mappings {
        return Err(ImportError::PersistenceFailure);
    }

    let mut final_seat_ids = plan
        .current_seats
        .iter()
        .map(|(code, seat)| (code.clone(), seat.seat_id().to_owned()))
        .collect::<BTreeMap<_, _>>();
    for seat_code in plan.diff.seats_removed() {
        let current = plan
            .current_seats
            .get(seat_code)
            .ok_or(ImportError::PersistenceFailure)?;
        if super::db::seats::delete_exact(transaction, current)? != 1 {
            return Err(ImportError::PersistenceFailure);
        }
        final_seat_ids.remove(seat_code);
    }
    for seat_code in plan.diff.seats_added() {
        let seat_id = Uuid::now_v7().to_string();
        if super::db::seats::insert(transaction, &seat_id, seat_code)? != 1 {
            return Err(ImportError::PersistenceFailure);
        }
        final_seat_ids.insert(seat_code.clone(), seat_id);
    }

    for (username, account) in &plan.current_accounts {
        if !plan.candidate_usernames.contains(username)
            && super::db::accounts::delete_exact(transaction, account)? != 1
        {
            return Err(ImportError::PersistenceFailure);
        }
    }

    let mut final_account_ids = BTreeMap::new();
    for row in sealed_rows {
        let account_id = if let Some(current) = plan.current_accounts.get(row.domjudge_username()) {
            let next = plan
                .next_credential_revisions
                .get(row.domjudge_username())
                .copied()
                .ok_or(ImportError::PersistenceFailure)?;
            if super::db::server_vault_records::update_account_credential(
                transaction,
                current,
                row,
            )? != 1
                || super::db::accounts::advance_credential_revision(transaction, current, next)?
                    != 1
            {
                return Err(ImportError::PersistenceFailure);
            }
            current.account_id().to_owned()
        } else {
            let account_id = Uuid::now_v7();
            if super::db::accounts::insert(transaction, account_id, row.domjudge_username())? != 1
                || super::db::server_vault_records::insert_account_credential(
                    transaction,
                    account_id,
                    row,
                )? != 1
            {
                return Err(ImportError::PersistenceFailure);
            }
            account_id.to_string()
        };
        final_account_ids.insert(row.domjudge_username().to_owned(), account_id);
    }

    for row in sealed_rows {
        let seat_id = final_seat_ids
            .get(row.seat_code())
            .ok_or(ImportError::PersistenceFailure)?;
        let account_id = final_account_ids
            .get(row.domjudge_username())
            .ok_or(ImportError::PersistenceFailure)?;
        if super::db::account_mappings::insert(transaction, seat_id, account_id)? != 1 {
            return Err(ImportError::PersistenceFailure);
        }
    }

    Ok(())
}

enum CommitOutcome {
    Committed,
    Unavailable,
    Stale,
    SeatOccupied,
}
