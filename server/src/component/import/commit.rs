use std::{collections::BTreeMap, sync::Arc};

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    db::{Database, Transaction, TransactionError},
    vault::VaultSession,
};

use super::{
    FINGERPRINT_VERSION,
    baseline::ImportBaseline,
    candidate::{CandidateExpiry, ImportError, expire_candidate},
    candidate_fingerprint, db,
    diff::{RedactedImportPreview, compute_diff},
    parse_roster,
    roster::{CandidateRoster, changed_passwords},
};

pub(super) async fn commit_import(
    database: &Database,
    vault: Arc<VaultSession>,
    candidate_id: Uuid,
    presented_token: &[u8; 32],
    raw_xlsx: &[u8],
) -> Result<(), ImportError> {
    let parsed = parse_roster(raw_xlsx).await?;
    let token_hash: [u8; 32] = Sha256::digest(presented_token).into();
    let outcome = database
        .write(move |transaction| {
            let Some(candidate) = db::pending_import_candidate::find(transaction)? else {
                return Ok(CommitOutcome::Unavailable);
            };
            if candidate.candidate_id() != candidate_id {
                return Ok(CommitOutcome::Unavailable);
            }
            if candidate.expiry() == CandidateExpiry::Expired {
                expire_candidate(transaction, &candidate)?;
                return Ok(CommitOutcome::Unavailable);
            }
            if !bool::from(token_hash.ct_eq(candidate.preview_token_hash())) {
                return Ok(CommitOutcome::Unavailable);
            }
            let baseline = db::query::read_baseline(transaction)?;
            if candidate.fingerprint_version() != FINGERPRINT_VERSION
                || candidate.baseline_fingerprint_sha256() != &baseline.fingerprint()
            {
                return Ok(CommitOutcome::Stale);
            }
            let roster = CandidateRoster::prepare(&baseline, &parsed)?;
            if candidate.candidate_fingerprint_sha256() != &candidate_fingerprint(&roster)? {
                return Ok(CommitOutcome::Stale);
            }
            let changed = changed_passwords(&baseline, &parsed, &vault)?;
            let diff = compute_diff(&baseline, &roster, changed);
            if candidate.diff() != &diff {
                return Ok(CommitOutcome::Stale);
            }
            if diff.blocks_commit() {
                return Ok(CommitOutcome::SeatOccupied);
            }
            if diff.has_changes() {
                apply_plan(transaction, &vault, &parsed, &baseline, &roster, &diff)?;
            }
            if db::pending_import_candidate::delete_exact(transaction, &candidate)? != 1 {
                return Err(ImportError::PersistenceFailure);
            }
            Ok(CommitOutcome::Committed)
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

fn require_one(count: usize) -> Result<(), ImportError> {
    if count != 1 {
        return Err(ImportError::PersistenceFailure);
    }
    Ok(())
}

fn apply_plan(
    transaction: &mut Transaction<'_>,
    vault: &VaultSession,
    parsed: &natsume_roster::Roster,
    baseline: &ImportBaseline,
    roster: &CandidateRoster,
    diff: &RedactedImportPreview,
) -> Result<(), ImportError> {
    // Remove all changed mappings before inserting replacements, allowing seat swaps.
    for code in diff
        .seats_removed
        .iter()
        .chain(diff.mappings_changed.iter().map(|change| &change.seat_code))
    {
        let seat = baseline
            .seats
            .get(code)
            .ok_or(ImportError::PersistenceFailure)?;
        if seat.current_domjudge_username().is_some() {
            require_one(db::account_mappings::delete_for_seat(
                transaction,
                seat.seat_id(),
            )?)?;
        }
    }
    let mut seat_ids = baseline
        .seats
        .iter()
        .map(|(code, seat)| (code.clone(), seat.seat_id().to_owned()))
        .collect::<BTreeMap<_, _>>();
    for code in &diff.seats_removed {
        let seat = baseline
            .seats
            .get(code)
            .ok_or(ImportError::PersistenceFailure)?;
        require_one(db::seats::delete_exact(transaction, seat)?)?;
        seat_ids.remove(code);
    }
    for code in &diff.seats_added {
        let id = Uuid::now_v7().to_string();
        require_one(db::seats::insert(transaction, &id, code)?)?;
        seat_ids.insert(code.clone(), id);
    }
    for account in &diff.accounts_removed {
        require_one(db::accounts::delete_exact(
            transaction,
            baseline
                .accounts
                .get(account)
                .ok_or(ImportError::PersistenceFailure)?,
        )?)?;
    }
    // Allocate in name order so first import matches the former preprocessor.
    for school in roster.organizations.values() {
        if baseline.organizations.get(school.key()) != Some(school) {
            require_one(db::organizations::save(transaction, school)?)?;
        }
    }
    for team in parsed.teams() {
        let id = save_account(transaction, vault, baseline, diff, team)?;
        let details = roster
            .teams
            .get(team.account())
            .ok_or(ImportError::PersistenceFailure)?;
        if baseline.teams.get(team.account()) != Some(details) {
            require_one(db::teams::save(transaction, &id, details)?)?;
        }
        if baseline
            .seats
            .get(team.seat())
            .and_then(|seat| seat.current_domjudge_username())
            != Some(team.account())
        {
            let seat_id = seat_ids
                .get(team.seat())
                .ok_or(ImportError::PersistenceFailure)?;
            require_one(db::account_mappings::insert(transaction, seat_id, &id)?)?;
        }
    }
    for school in baseline.organizations.values() {
        if !roster.organizations.contains_key(school.key()) {
            require_one(db::organizations::delete(
                transaction,
                school.organization_id,
            )?)?;
        }
    }
    Ok(())
}

fn save_account(
    transaction: &mut Transaction<'_>,
    vault: &VaultSession,
    baseline: &ImportBaseline,
    diff: &RedactedImportPreview,
    team: &natsume_roster::Team,
) -> Result<String, ImportError> {
    if let Some(current) = baseline.accounts.get(team.account()) {
        if diff
            .passwords_changed
            .binary_search_by(|account| account.as_str().cmp(team.account()))
            .is_ok()
        {
            let next = current
                .credential_revision()
                .checked_add(1)
                .ok_or(ImportError::PersistenceFailure)?;
            let (nonce, ciphertext) = vault
                .seal(team.password().as_bytes())
                .map_err(|_| ImportError::VaultFailure)?;
            require_one(db::server_vault_records::save(
                transaction,
                current.account_id(),
                &nonce,
                &ciphertext,
            )?)?;
            require_one(db::accounts::advance_credential_revision(
                transaction,
                current,
                next,
            )?)?;
        }
        Ok(current.account_id().to_owned())
    } else {
        let id = Uuid::now_v7();
        require_one(db::accounts::insert(transaction, id, team.account())?)?;
        let (nonce, ciphertext) = vault
            .seal(team.password().as_bytes())
            .map_err(|_| ImportError::VaultFailure)?;
        require_one(db::server_vault_records::save(
            transaction,
            &id.to_string(),
            &nonce,
            &ciphertext,
        )?)?;
        Ok(id.to_string())
    }
}

enum CommitOutcome {
    Committed,
    Unavailable,
    Stale,
    SeatOccupied,
}
