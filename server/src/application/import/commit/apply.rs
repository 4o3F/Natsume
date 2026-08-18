use std::collections::BTreeMap;

use crate::{
    application::contest::{CurrentSeatProjection, NewAccountFacts},
    audit::ImportCommitAuditFacts,
    db::{self},
};

use super::super::{ImportError, SealedCommitRow};
use super::plan::CommitPlan;

pub(super) fn apply_commit_plan(
    transaction: &mut db::Transaction<'_>,
    sealed_rows: &[SealedCommitRow],
    plan: &CommitPlan,
) -> Result<CommitMutationFacts, ImportError> {
    delete_current_mappings(transaction, plan)?;
    apply_seat_mutations(transaction, plan)?;
    remove_absent_accounts(transaction, plan)?;
    let final_account_ids = upsert_account_credentials(transaction, sealed_rows, plan)?;
    insert_final_mappings(transaction, sealed_rows, plan, &final_account_ids)?;
    advance_revisions(transaction, plan)?;

    Ok(CommitMutationFacts {
        configuration_revision: plan.next_configuration_revision,
        binding_revision: plan.next_binding_revision,
        seats_added_count: plan.diff.seats_added().len(),
        seats_removed_count: plan.diff.seats_removed().len(),
        mappings_changed_count: plan.diff.mappings_changed().len(),
        binding_impact_count: plan.diff.binding_impacts().len(),
        credential_revision_advanced_count: sealed_rows.len(),
        configuration_revision_advanced: plan.configuration_revision_advanced,
        binding_revision_advanced: plan.binding_revision_advanced,
    })
}

fn delete_current_mappings(
    transaction: &mut db::Transaction<'_>,
    plan: &CommitPlan,
) -> Result<(), ImportError> {
    let removed_mapping_count = db::contest::account_mappings::delete_all(transaction)
        .map_err(ImportError::from_contest_persistence)?;
    let expected_mapping_count = plan
        .current_seats
        .values()
        .filter(|facts| facts.current_domjudge_username().is_some())
        .count();
    if removed_mapping_count != expected_mapping_count {
        return Err(ImportError::PersistenceFailure);
    }
    Ok(())
}

fn apply_seat_mutations(
    transaction: &mut db::Transaction<'_>,
    plan: &CommitPlan,
) -> Result<(), ImportError> {
    for seat_code in plan.diff.seats_removed() {
        let current = plan
            .current_seats
            .get(seat_code)
            .ok_or(ImportError::PersistenceFailure)?;
        let removed_binding = if current.device_id().is_some() {
            db::contest::device_bindings::delete_by_seat(transaction, current.seat_id())
                .map_err(ImportError::from_contest_persistence)?
        } else {
            0
        };
        if removed_binding != usize::from(current.device_id().is_some()) {
            return Err(ImportError::PersistenceFailure);
        }
        let removed_seat = db::contest::seats::delete_exact(transaction, current)
            .map_err(ImportError::from_contest_persistence)?;
        if removed_seat != 1 {
            return Err(ImportError::PersistenceFailure);
        }
    }
    for seat_code in plan.diff.seats_added() {
        let inserted = db::contest::seats::insert(transaction, seat_code, seat_code)
            .map_err(ImportError::from_contest_persistence)?;
        if inserted != 1 {
            return Err(ImportError::PersistenceFailure);
        }
    }
    Ok(())
}

fn remove_absent_accounts(
    transaction: &mut db::Transaction<'_>,
    plan: &CommitPlan,
) -> Result<(), ImportError> {
    for (username, current) in &plan.current_accounts {
        if plan.candidate_usernames.contains(username) {
            continue;
        }
        let removed_account = db::contest::accounts::delete_exact(transaction, current)
            .map_err(ImportError::from_contest_persistence)?;
        let removed_vault =
            db::import::server_vault_records::delete_account_credential(transaction, current)?;
        if removed_account != 1 || removed_vault != 1 {
            return Err(ImportError::PersistenceFailure);
        }
    }
    Ok(())
}

fn upsert_account_credentials(
    transaction: &mut db::Transaction<'_>,
    sealed_rows: &[SealedCommitRow],
    plan: &CommitPlan,
) -> Result<BTreeMap<String, String>, ImportError> {
    let mut final_account_ids = BTreeMap::new();
    for row in sealed_rows {
        let account_id = if let Some(current) = plan.current_accounts.get(row.domjudge_username()) {
            let updated_vault = db::import::server_vault_records::update_account_credential(
                transaction,
                current,
                row,
            )?;
            let next_revision = plan
                .next_credential_revisions
                .get(row.domjudge_username())
                .copied()
                .ok_or(ImportError::PersistenceFailure)?;
            let updated_account = db::contest::accounts::advance_credential_revision(
                transaction,
                current,
                next_revision,
            )
            .map_err(ImportError::from_contest_persistence)?;
            if updated_vault != 1 || updated_account != 1 {
                return Err(ImportError::PersistenceFailure);
            }
            current.account_id().to_owned()
        } else {
            let new_account = NewAccountFacts::new(row.domjudge_username().to_owned());
            let inserted_vault = db::import::server_vault_records::insert_account_credential(
                transaction,
                &new_account,
                row,
            )?;
            let inserted_account = db::contest::accounts::insert(transaction, &new_account)
                .map_err(ImportError::from_contest_persistence)?;
            if inserted_vault != 1 || inserted_account != 1 {
                return Err(ImportError::PersistenceFailure);
            }
            new_account.account_id().to_string()
        };
        final_account_ids.insert(row.domjudge_username().to_owned(), account_id);
    }
    Ok(final_account_ids)
}

fn insert_final_mappings(
    transaction: &mut db::Transaction<'_>,
    sealed_rows: &[SealedCommitRow],
    plan: &CommitPlan,
    final_account_ids: &BTreeMap<String, String>,
) -> Result<(), ImportError> {
    for row in sealed_rows {
        let seat_id = plan
            .current_seats
            .get(row.seat_code())
            .map_or(row.seat_code(), CurrentSeatProjection::seat_id);
        let account_id = final_account_ids
            .get(row.domjudge_username())
            .ok_or(ImportError::PersistenceFailure)?;
        let inserted = db::contest::account_mappings::insert(transaction, seat_id, account_id)
            .map_err(ImportError::from_contest_persistence)?;
        if inserted != 1 {
            return Err(ImportError::PersistenceFailure);
        }
    }
    Ok(())
}

fn advance_revisions(
    transaction: &mut db::Transaction<'_>,
    plan: &CommitPlan,
) -> Result<(), ImportError> {
    if plan.configuration_revision_advanced || plan.binding_revision_advanced {
        let advanced = db::import::revision_counters::advance(
            transaction,
            plan.baseline_configuration_revision,
            plan.baseline_binding_revision,
            plan.next_configuration_revision,
            plan.next_binding_revision,
        )
        .map_err(ImportError::from_contest_persistence)?;
        if advanced != 1 {
            return Err(ImportError::PersistenceFailure);
        }
    }
    Ok(())
}

pub(super) struct CommitMutationFacts {
    pub(super) configuration_revision: i64,
    pub(super) binding_revision: i64,
    pub(super) seats_added_count: usize,
    pub(super) seats_removed_count: usize,
    pub(super) mappings_changed_count: usize,
    pub(super) binding_impact_count: usize,
    pub(super) credential_revision_advanced_count: usize,
    pub(super) configuration_revision_advanced: bool,
    pub(super) binding_revision_advanced: bool,
}

impl CommitMutationFacts {
    pub(super) const fn audit_facts(&self) -> ImportCommitAuditFacts {
        ImportCommitAuditFacts {
            seats_added_count: self.seats_added_count,
            seats_removed_count: self.seats_removed_count,
            mappings_changed_count: self.mappings_changed_count,
            binding_impact_count: self.binding_impact_count,
            credential_revision_advanced_count: self.credential_revision_advanced_count,
            configuration_revision_advanced: self.configuration_revision_advanced,
            binding_revision_advanced: self.binding_revision_advanced,
        }
    }
}
