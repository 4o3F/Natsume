use std::collections::{BTreeMap, BTreeSet};

use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    audit::{
        AuditEvent, AuditEventId, CorrelationId, ImportCommitAuditFacts,
        ImportCommitRejectionReason,
    },
    component::contest::{CurrentAccountProjection, CurrentSeatProjection, NewAccountFacts},
    db::{Database, Transaction},
    vault::VaultSession,
};

use super::{
    FINGERPRINT_VERSION,
    candidate::{CandidateExpiry, ImportError, PreviewToken, SealedCommitRow, expire_candidate},
    candidate_fingerprint,
    csv::parse_csv,
    current_fingerprint,
    diff::{RedactedImportPreview, compute_diff},
    seal_rows,
};

pub(crate) async fn commit_import(
    database: &Database,
    vault: &VaultSession,
    candidate_id: Uuid,
    presented_token: &PreviewToken,
    raw_csv: &[u8],
    correlation_id: CorrelationId,
) -> Result<(), ImportError> {
    let parsed = parse_csv(raw_csv).map_err(ImportError::InvalidCsv)?;
    let candidate_rows = parsed.candidate_rows();
    let candidate_hash = candidate_fingerprint(&candidate_rows);
    let sealed_rows = seal_rows(vault, &parsed.rows)?;
    drop(parsed);

    let token_hash = presented_token.sha256();
    let expiry_audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    let commit_audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    let outcome = database
        .write(move |transaction| {
            commit_in_transaction(
                transaction,
                candidate_id,
                token_hash,
                candidate_hash,
                &candidate_rows,
                &sealed_rows,
                correlation_id,
                expiry_audit_event_id,
                commit_audit_event_id,
            )
        })
        .await?;

    match outcome {
        CommitOutcome::Committed => Ok(()),
        CommitOutcome::Unavailable => Err(ImportError::CandidateUnavailable),
        CommitOutcome::Stale => Err(ImportError::PreviewStale),
        CommitOutcome::SeatOccupied => Err(ImportError::SeatOccupied),
    }
}

#[allow(clippy::too_many_arguments)]
fn commit_in_transaction(
    transaction: &mut Transaction<'_>,
    candidate_id: Uuid,
    token_hash: [u8; 32],
    candidate_hash: [u8; 32],
    candidate_rows: &[super::candidate::CandidateRowFacts],
    sealed_rows: &[SealedCommitRow],
    correlation_id: CorrelationId,
    expiry_audit_event_id: AuditEventId,
    commit_audit_event_id: AuditEventId,
) -> Result<CommitOutcome, ImportError> {
    let Some(candidate) = super::db::pending_import_candidate::find(transaction)? else {
        return Ok(CommitOutcome::Unavailable);
    };
    if candidate.candidate_id() != candidate_id {
        return Ok(CommitOutcome::Unavailable);
    }
    if candidate.expiry() == CandidateExpiry::Expired {
        expire_candidate(
            transaction,
            &candidate,
            correlation_id,
            expiry_audit_event_id,
        )?;
        return Ok(CommitOutcome::Unavailable);
    }
    if !bool::from(
        token_hash
            .as_slice()
            .ct_eq(candidate.preview_token_hash().as_slice()),
    ) {
        insert_rejection(
            transaction,
            commit_audit_event_id,
            correlation_id,
            candidate_id,
            ImportCommitRejectionReason::PreviewTokenMismatch,
        )?;
        return Ok(CommitOutcome::Unavailable);
    }
    if candidate.fingerprint_version() != FINGERPRINT_VERSION
        || candidate.candidate_fingerprint_sha256() != &candidate_hash
    {
        insert_rejection(
            transaction,
            commit_audit_event_id,
            correlation_id,
            candidate_id,
            ImportCommitRejectionReason::CandidateChanged,
        )?;
        return Ok(CommitOutcome::Stale);
    }

    let current_seats = super::db::query::read_current_seats(transaction)?;
    let current_accounts = super::db::query::read_current_accounts(transaction)?;
    let baseline_hash = current_fingerprint(&current_seats, &current_accounts);
    if candidate.baseline_fingerprint_sha256() != &baseline_hash {
        insert_rejection(
            transaction,
            commit_audit_event_id,
            correlation_id,
            candidate_id,
            ImportCommitRejectionReason::BaselineStale,
        )?;
        return Ok(CommitOutcome::Stale);
    }

    let plan = CommitPlan::prepare(current_seats, current_accounts, candidate_rows)?;
    if !plan.diff.binding_impacts().is_empty() {
        insert_rejection(
            transaction,
            commit_audit_event_id,
            correlation_id,
            candidate_id,
            ImportCommitRejectionReason::SeatOccupied,
        )?;
        return Ok(CommitOutcome::SeatOccupied);
    }
    let audit_facts = apply_plan(transaction, sealed_rows, &plan)?;
    if super::db::pending_import_candidate::delete_exact(transaction, &candidate)? != 1 {
        return Err(ImportError::PersistenceFailure);
    }
    let event = AuditEvent::import_committed(
        commit_audit_event_id,
        correlation_id,
        candidate_id,
        &audit_facts,
    );
    crate::audit::insert(transaction, &event).map_err(ImportError::from_audit_persistence)?;
    Ok(CommitOutcome::Committed)
}

fn insert_rejection(
    transaction: &mut Transaction<'_>,
    audit_event_id: AuditEventId,
    correlation_id: CorrelationId,
    candidate_id: Uuid,
    reason: ImportCommitRejectionReason,
) -> Result<(), ImportError> {
    let event =
        AuditEvent::import_commit_rejected(audit_event_id, correlation_id, candidate_id, reason);
    crate::audit::insert(transaction, &event).map_err(ImportError::from_audit_persistence)
}

struct CommitPlan {
    current_seats: BTreeMap<String, CurrentSeatProjection>,
    current_accounts: BTreeMap<String, CurrentAccountProjection>,
    candidate_usernames: BTreeSet<String>,
    next_credential_revisions: BTreeMap<String, i64>,
    diff: RedactedImportPreview,
}

impl CommitPlan {
    fn prepare(
        current_seats: Vec<CurrentSeatProjection>,
        current_accounts: Vec<CurrentAccountProjection>,
        candidate_rows: &[super::candidate::CandidateRowFacts],
    ) -> Result<Self, ImportError> {
        let diff = compute_diff(&current_seats, candidate_rows)?;
        let mut seats = BTreeMap::new();
        for seat in current_seats {
            if seats.insert(seat.seat_code().to_owned(), seat).is_some() {
                return Err(ImportError::PersistenceFailure);
            }
        }
        let mut accounts = BTreeMap::new();
        for account in current_accounts {
            if account.credential_revision() < 1
                || accounts
                    .insert(account.domjudge_username().to_owned(), account)
                    .is_some()
            {
                return Err(ImportError::PersistenceFailure);
            }
        }
        let candidate_usernames = candidate_rows
            .iter()
            .map(|row| row.domjudge_username().to_owned())
            .collect::<BTreeSet<_>>();
        let mut next_credential_revisions = BTreeMap::new();
        for username in &candidate_usernames {
            if let Some(account) = accounts.get(username) {
                let next = account
                    .credential_revision()
                    .checked_add(1)
                    .ok_or(ImportError::PersistenceFailure)?;
                next_credential_revisions.insert(username.clone(), next);
            }
        }
        Ok(Self {
            current_seats: seats,
            current_accounts: accounts,
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
) -> Result<ImportCommitAuditFacts, ImportError> {
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
            let account = NewAccountFacts::new(row.domjudge_username().to_owned());
            if super::db::accounts::insert(transaction, &account)? != 1
                || super::db::server_vault_records::insert_account_credential(
                    transaction,
                    &account,
                    row,
                )? != 1
            {
                return Err(ImportError::PersistenceFailure);
            }
            account.account_id().to_string()
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

    Ok(ImportCommitAuditFacts {
        seats_added: plan.diff.seats_added().len(),
        seats_removed: plan.diff.seats_removed().len(),
        mappings_changed: plan.diff.mappings_changed().len(),
        binding_impacts: 0,
        credentials_advanced: sealed_rows.len(),
    })
}

enum CommitOutcome {
    Committed,
    Unavailable,
    Stale,
    SeatOccupied,
}
