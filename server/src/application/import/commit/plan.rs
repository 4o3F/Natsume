use std::collections::{BTreeMap, BTreeSet};

use super::super::{
    CandidateRowFacts, ImportError, SealedCommitRow,
    csv::{ACCOUNT_USERNAME_LENGTH_LIMIT, MAX_IMPORT_ROWS, SEAT_CODE_LENGTH_LIMIT},
    diff::{CurrentAccountProjection, CurrentSeatProjection, RedactedImportPreview, compute_diff},
};

pub(super) struct CommitPlan {
    pub(super) current_seats: BTreeMap<String, CurrentSeatProjection>,
    pub(super) current_accounts: BTreeMap<String, CurrentAccountProjection>,
    pub(super) candidate_usernames: BTreeSet<String>,
    pub(super) next_credential_revisions: BTreeMap<String, i64>,
    pub(super) diff: RedactedImportPreview,
    pub(super) baseline_configuration_revision: i64,
    pub(super) baseline_binding_revision: i64,
    pub(super) next_configuration_revision: i64,
    pub(super) next_binding_revision: i64,
    pub(super) configuration_revision_advanced: bool,
    pub(super) binding_revision_advanced: bool,
}

pub(super) fn prepare_commit_plan(
    current_seat_rows: Vec<CurrentSeatProjection>,
    current_account_rows: Vec<CurrentAccountProjection>,
    sealed_rows: &[SealedCommitRow],
    baseline_configuration_revision: i64,
    baseline_binding_revision: i64,
) -> Result<CommitPlan, ImportError> {
    if sealed_rows.is_empty() || sealed_rows.len() > MAX_IMPORT_ROWS {
        return Err(ImportError::PersistenceFailure);
    }
    let mut candidate_rows = Vec::with_capacity(sealed_rows.len());
    let mut candidate_seats = BTreeSet::new();
    let mut candidate_usernames = BTreeSet::new();
    for row in sealed_rows {
        if !valid_commit_field(row.seat_code(), SEAT_CODE_LENGTH_LIMIT)
            || !valid_commit_field(row.domjudge_username(), ACCOUNT_USERNAME_LENGTH_LIMIT)
            || row.ciphertext().is_empty()
            || !candidate_seats.insert(row.seat_code().to_owned())
            || !candidate_usernames.insert(row.domjudge_username().to_owned())
        {
            return Err(ImportError::PersistenceFailure);
        }
        candidate_rows.push(CandidateRowFacts {
            seat_code: row.seat_code().to_owned(),
            domjudge_username: row.domjudge_username().to_owned(),
        });
    }

    let diff = compute_diff(&current_seat_rows, &candidate_rows)
        .map_err(|_| ImportError::PersistenceFailure)?;
    let current_seats = index_current_seats(current_seat_rows)?;
    let current_accounts = index_current_accounts(current_account_rows)?;
    let mut next_credential_revisions = BTreeMap::new();
    for username in &candidate_usernames {
        if let Some(current) = current_accounts.get(username) {
            let next = current
                .credential_revision()
                .checked_add(1)
                .ok_or(ImportError::PersistenceFailure)?;
            next_credential_revisions.insert(username.clone(), next);
        }
    }

    let configuration_revision_advanced = !diff.seats_added().is_empty()
        || !diff.seats_removed().is_empty()
        || !diff.mappings_changed().is_empty();
    let binding_revision_advanced = !diff.binding_impacts().is_empty();
    let next_configuration_revision = if configuration_revision_advanced {
        baseline_configuration_revision
            .checked_add(1)
            .ok_or(ImportError::PersistenceFailure)?
    } else {
        baseline_configuration_revision
    };
    let next_binding_revision = if binding_revision_advanced {
        baseline_binding_revision
            .checked_add(1)
            .ok_or(ImportError::PersistenceFailure)?
    } else {
        baseline_binding_revision
    };

    Ok(CommitPlan {
        current_seats,
        current_accounts,
        candidate_usernames,
        next_credential_revisions,
        diff,
        baseline_configuration_revision,
        baseline_binding_revision,
        next_configuration_revision,
        next_binding_revision,
        configuration_revision_advanced,
        binding_revision_advanced,
    })
}

fn index_current_seats(
    rows: Vec<CurrentSeatProjection>,
) -> Result<BTreeMap<String, CurrentSeatProjection>, ImportError> {
    let mut indexed = BTreeMap::new();
    for row in rows {
        if indexed.insert(row.seat_code().to_owned(), row).is_some() {
            return Err(ImportError::PersistenceFailure);
        }
    }
    Ok(indexed)
}

fn index_current_accounts(
    rows: Vec<CurrentAccountProjection>,
) -> Result<BTreeMap<String, CurrentAccountProjection>, ImportError> {
    let mut indexed = BTreeMap::new();
    for row in rows {
        if row.credential_revision() < 1
            || indexed
                .insert(row.domjudge_username().to_owned(), row)
                .is_some()
        {
            return Err(ImportError::PersistenceFailure);
        }
    }
    Ok(indexed)
}

fn valid_commit_field(value: &str, length_limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= length_limit
        && !value.contains(',')
        && !value.chars().any(char::is_control)
}
