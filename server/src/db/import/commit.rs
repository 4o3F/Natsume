use std::collections::{BTreeMap, BTreeSet};

use diesel::{
    ExpressionMethods, JoinOnDsl, NullableExpressionMethods, QueryDsl, RunQueryDsl,
    sql_types::{BigInt, Integer},
    sqlite::SqliteConnection,
};
use uuid::Uuid;

use crate::{
    application::import::{
        CandidateRowFacts, CommittedImportFacts, ImportError, MAX_IMPORT_ROWS,
        RedactedImportPreview, SealedCommitRow,
    },
    audit::{
        self, AuditEvent, AuditEventId, CorrelationId, ImportCommitAuditFacts,
        ImportCommitRejectionReason,
    },
    db::{
        Database,
        schema::{
            account_mappings, accounts, device_bindings, pending_import_candidate,
            revision_counters, seats, server_vault_records,
        },
    },
    vault::VaultRecordType,
};

use super::{
    ImportStoreError,
    candidate::{
        PendingCandidateRow, expire_pending_candidate_tolerant, pending_preview_token_hash,
        read_pending_candidate,
    },
    diff::{CurrentSeatFacts, compute_diff, read_current_seats, read_revision_counters},
};

pub(crate) async fn commit_import(
    database: &Database,
    candidate_id: Uuid,
    expected_preview_token_hash: [u8; 32],
    expected_payload_vault_record_id: Uuid,
    sealed_rows: Vec<SealedCommitRow>,
    correlation_id: CorrelationId,
) -> Result<CommittedImportFacts, ImportError> {
    let request = CommitRequest {
        candidate_id,
        expected_preview_token_hash,
        expected_payload_vault_record_id,
        sealed_rows,
        correlation_id,
    };
    let outcome = commit_import_with_ids(
        database,
        request,
        AuditEventId::from_uuid(Uuid::now_v7()),
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(ImportError::from)?;
    match outcome {
        CommitOutcome::Committed(facts) => Ok(facts),
        CommitOutcome::Unavailable => Err(ImportError::CandidateUnavailable),
        CommitOutcome::Stale => Err(ImportError::PreviewStale),
    }
}

pub(super) struct CommitRequest {
    pub(super) candidate_id: Uuid,
    pub(super) expected_preview_token_hash: [u8; 32],
    pub(super) expected_payload_vault_record_id: Uuid,
    pub(super) sealed_rows: Vec<SealedCommitRow>,
    pub(super) correlation_id: CorrelationId,
}

pub(super) enum CommitOutcome {
    Committed(CommittedImportFacts),
    Unavailable,
    Stale,
}

pub(super) async fn commit_import_with_ids(
    database: &Database,
    request: CommitRequest,
    expiry_audit_event_id: AuditEventId,
    commit_audit_event_id: AuditEventId,
) -> Result<CommitOutcome, ImportStoreError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                let Some(pending) = read_pending_candidate(connection)? else {
                    return Ok(CommitOutcome::Unavailable);
                };
                if pending.candidate_id != request.candidate_id.to_string() {
                    return Ok(CommitOutcome::Unavailable);
                }
                match pending.expiry_state {
                    1 => {
                        expire_pending_candidate_tolerant(
                            connection,
                            &pending,
                            request.candidate_id,
                            request.correlation_id,
                            expiry_audit_event_id,
                        )?;
                        return Ok(CommitOutcome::Unavailable);
                    }
                    0 => {}
                    _ => return Err(ImportStoreError::InvalidPersistedFacts),
                }
                if pending_preview_token_hash(&pending)? != request.expected_preview_token_hash
                    || pending.payload_vault_record_id
                        != request.expected_payload_vault_record_id.to_string()
                {
                    return Ok(CommitOutcome::Unavailable);
                }

                let (configuration_revision, binding_revision) =
                    read_revision_counters(connection)?;
                if configuration_revision != pending.baseline_configuration_revision
                    || binding_revision != pending.baseline_binding_revision
                {
                    let event = AuditEvent::import_commit_rejected(
                        commit_audit_event_id,
                        request.correlation_id,
                        request.candidate_id,
                        ImportCommitRejectionReason::BaselineStale,
                    );
                    // §3.4 keeps stale classification stable under rejected-audit failure.
                    if audit::insert_diesel(connection, &event).is_err() {
                        tracing::warn!(
                            discriminant = "baseline_stale_audit_write_failed",
                            "import rejection audit write failed"
                        );
                    }
                    return Ok(CommitOutcome::Stale);
                }

                let mutation = apply_committed_candidate(
                    connection,
                    &request.sealed_rows,
                    pending.baseline_configuration_revision,
                    pending.baseline_binding_revision,
                )?;
                delete_committed_candidate(connection, &pending, request.candidate_id)?;
                let event = AuditEvent::import_committed(
                    commit_audit_event_id,
                    request.correlation_id,
                    request.candidate_id,
                    &ImportCommitAuditFacts {
                        seats_added_count: mutation.seats_added_count,
                        seats_removed_count: mutation.seats_removed_count,
                        mappings_changed_count: mutation.mappings_changed_count,
                        binding_impact_count: mutation.binding_impact_count,
                        credential_revision_advanced_count: mutation
                            .credential_revision_advanced_count,
                        configuration_revision_advanced: mutation.configuration_revision_advanced,
                        binding_revision_advanced: mutation.binding_revision_advanced,
                    },
                );
                audit::insert_diesel(connection, &event)
                    .map_err(|_| ImportStoreError::AuditInsertFailed)?;
                Ok(CommitOutcome::Committed(CommittedImportFacts::new(
                    mutation.configuration_revision,
                    mutation.binding_revision,
                )))
            })
        })
        .await
        .map_err(|_| ImportStoreError::AcquireFailed)?
}

pub(super) fn delete_committed_candidate(
    connection: &mut SqliteConnection,
    pending: &PendingCandidateRow,
    candidate_id: Uuid,
) -> Result<(), ImportStoreError> {
    let removed_candidate = diesel::delete(
        pending_import_candidate::table
            .filter(pending_import_candidate::singleton.eq(Some(1_i32)))
            .filter(pending_import_candidate::candidate_id.eq(candidate_id.to_string())),
    )
    .execute(connection)
    .map_err(|_| ImportStoreError::CandidateDeleteFailed)?;
    let removed_payload = diesel::delete(
        server_vault_records::table
            .filter(server_vault_records::vault_record_id.eq(&pending.payload_vault_record_id))
            .filter(server_vault_records::record_type.eq(VaultRecordType::ImportPayload.as_str()))
            .filter(server_vault_records::subject_id.eq(candidate_id.to_string())),
    )
    .execute(connection)
    .map_err(|_| ImportStoreError::VaultDeleteFailed)?;
    if removed_candidate != 1 || removed_payload != 1 {
        return Err(ImportStoreError::MutationConflict);
    }
    Ok(())
}

pub(super) struct CurrentAccountFacts {
    pub(super) account_id: String,
    pub(super) credential_vault_record_id: String,
    pub(super) credential_revision: i64,
}

pub(super) fn read_current_accounts(
    connection: &mut SqliteConnection,
) -> Result<BTreeMap<String, CurrentAccountFacts>, ImportStoreError> {
    let rows = accounts::table
        .left_join(
            server_vault_records::table
                .on(accounts::credential_vault_record_id.eq(server_vault_records::vault_record_id)),
        )
        .select((
            accounts::account_id,
            accounts::domjudge_username,
            accounts::credential_vault_record_id,
            diesel::dsl::sql::<BigInt>("credential_revision"),
            server_vault_records::record_type.nullable(),
            server_vault_records::subject_id.nullable(),
        ))
        .order(accounts::domjudge_username)
        .load::<(String, String, String, i64, Option<String>, Option<String>)>(connection)
        .map_err(|_| ImportStoreError::CurrentFactsReadFailed)?;

    let mut accounts_by_username = BTreeMap::new();
    for (
        account_id,
        domjudge_username,
        credential_vault_record_id,
        credential_revision,
        record_type,
        subject_id,
    ) in rows
    {
        if credential_revision < 1
            || record_type.as_deref() != Some(VaultRecordType::AccountCredential.as_str())
            || subject_id.as_deref() != Some(account_id.as_str())
            || accounts_by_username
                .insert(
                    domjudge_username,
                    CurrentAccountFacts {
                        account_id,
                        credential_vault_record_id,
                        credential_revision,
                    },
                )
                .is_some()
        {
            return Err(ImportStoreError::InvalidPersistedFacts);
        }
    }
    Ok(accounts_by_username)
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

pub(super) fn apply_committed_candidate(
    connection: &mut SqliteConnection,
    sealed_rows: &[SealedCommitRow],
    baseline_configuration_revision: i64,
    baseline_binding_revision: i64,
) -> Result<CommitMutationFacts, ImportStoreError> {
    let plan = prepare_commit_plan(
        connection,
        sealed_rows,
        baseline_configuration_revision,
        baseline_binding_revision,
    )?;
    replace_seats_and_clear_mappings(connection, &plan)?;
    let final_account_ids = replace_accounts(connection, sealed_rows, &plan)?;
    insert_final_mappings(
        connection,
        sealed_rows,
        &plan.current_seats,
        &final_account_ids,
    )?;
    persist_revisions(connection, &plan)?;
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

pub(super) struct CommitPlan {
    pub(super) current_seats: BTreeMap<String, CurrentSeatFacts>,
    pub(super) current_accounts: BTreeMap<String, CurrentAccountFacts>,
    pub(super) candidate_usernames: BTreeSet<String>,
    pub(super) diff: RedactedImportPreview,
    pub(super) baseline_configuration_revision: i64,
    pub(super) baseline_binding_revision: i64,
    pub(super) next_configuration_revision: i64,
    pub(super) next_binding_revision: i64,
    pub(super) configuration_revision_advanced: bool,
    pub(super) binding_revision_advanced: bool,
}

pub(super) fn prepare_commit_plan(
    connection: &mut SqliteConnection,
    sealed_rows: &[SealedCommitRow],
    baseline_configuration_revision: i64,
    baseline_binding_revision: i64,
) -> Result<CommitPlan, ImportStoreError> {
    if sealed_rows.is_empty() || sealed_rows.len() > MAX_IMPORT_ROWS {
        return Err(ImportStoreError::InvalidPersistedFacts);
    }
    let mut candidate_rows = Vec::with_capacity(sealed_rows.len());
    let mut candidate_usernames = BTreeSet::new();
    for row in sealed_rows {
        if row.seat_code.is_empty()
            || row.domjudge_username.is_empty()
            || row.ciphertext.is_empty()
            || !candidate_usernames.insert(row.domjudge_username.clone())
        {
            return Err(ImportStoreError::InvalidPersistedFacts);
        }
        candidate_rows.push(CandidateRowFacts {
            seat_code: row.seat_code.clone(),
            domjudge_username: row.domjudge_username.clone(),
        });
    }
    let current_seats = read_current_seats(connection)?;
    let current_accounts = read_current_accounts(connection)?;
    let diff = compute_diff(&current_seats, &candidate_rows)
        .map_err(|_| ImportStoreError::InvalidPersistedFacts)?;
    let configuration_revision_advanced = !diff.seats_added().is_empty()
        || !diff.seats_removed().is_empty()
        || !diff.mappings_changed().is_empty();
    let binding_revision_advanced = !diff.binding_impacts().is_empty();
    let next_configuration_revision = if configuration_revision_advanced {
        baseline_configuration_revision
            .checked_add(1)
            .ok_or(ImportStoreError::RevisionOverflow)?
    } else {
        baseline_configuration_revision
    };
    let next_binding_revision = if binding_revision_advanced {
        baseline_binding_revision
            .checked_add(1)
            .ok_or(ImportStoreError::RevisionOverflow)?
    } else {
        baseline_binding_revision
    };
    Ok(CommitPlan {
        current_seats,
        current_accounts,
        candidate_usernames,
        diff,
        baseline_configuration_revision,
        baseline_binding_revision,
        next_configuration_revision,
        next_binding_revision,
        configuration_revision_advanced,
        binding_revision_advanced,
    })
}

pub(super) fn replace_seats_and_clear_mappings(
    connection: &mut SqliteConnection,
    plan: &CommitPlan,
) -> Result<(), ImportStoreError> {
    let removed_mapping_count = diesel::delete(account_mappings::table)
        .execute(connection)
        .map_err(|_| ImportStoreError::MutationFailed)?;
    let current_mapping_count = plan
        .current_seats
        .values()
        .filter(|facts| facts.current_domjudge_username.is_some())
        .count();
    if removed_mapping_count != current_mapping_count {
        return Err(ImportStoreError::MutationConflict);
    }
    for seat_code in plan.diff.seats_removed() {
        let facts = plan
            .current_seats
            .get(seat_code)
            .ok_or(ImportStoreError::InvalidPersistedFacts)?;
        let removed_binding = diesel::delete(
            device_bindings::table.filter(device_bindings::seat_id.eq(&facts.seat_id)),
        )
        .execute(connection)
        .map_err(|_| ImportStoreError::MutationFailed)?;
        if removed_binding != usize::from(facts.device_id.is_some()) {
            return Err(ImportStoreError::MutationConflict);
        }
        let removed_seat = diesel::delete(seats::table.filter(seats::seat_id.eq(&facts.seat_id)))
            .execute(connection)
            .map_err(|_| ImportStoreError::MutationFailed)?;
        if removed_seat != 1 {
            return Err(ImportStoreError::MutationConflict);
        }
    }
    for seat_code in plan.diff.seats_added() {
        let inserted = diesel::insert_into(seats::table)
            .values((seats::seat_id.eq(seat_code), seats::seat_code.eq(seat_code)))
            .execute(connection)
            .map_err(|_| ImportStoreError::MutationFailed)?;
        if inserted != 1 {
            return Err(ImportStoreError::MutationConflict);
        }
    }
    Ok(())
}

pub(super) fn replace_accounts(
    connection: &mut SqliteConnection,
    sealed_rows: &[SealedCommitRow],
    plan: &CommitPlan,
) -> Result<BTreeMap<String, String>, ImportStoreError> {
    remove_absent_accounts(
        connection,
        &plan.current_accounts,
        &plan.candidate_usernames,
    )?;
    let mut final_account_ids = BTreeMap::new();
    for row in sealed_rows {
        let account_id = if let Some(current) = plan.current_accounts.get(&row.domjudge_username) {
            rotate_account_credential(connection, current, row)?
        } else {
            create_account_credential(connection, row)?
        };
        final_account_ids.insert(row.domjudge_username.clone(), account_id);
    }
    Ok(final_account_ids)
}

pub(super) fn remove_absent_accounts(
    connection: &mut SqliteConnection,
    current_accounts: &BTreeMap<String, CurrentAccountFacts>,
    candidate_usernames: &BTreeSet<String>,
) -> Result<(), ImportStoreError> {
    for (username, facts) in current_accounts {
        if candidate_usernames.contains(username) {
            continue;
        }
        let removed_account =
            diesel::delete(accounts::table.filter(accounts::account_id.eq(&facts.account_id)))
                .execute(connection)
                .map_err(|_| ImportStoreError::MutationFailed)?;
        let removed_vault = diesel::delete(
            server_vault_records::table
                .filter(server_vault_records::vault_record_id.eq(&facts.credential_vault_record_id))
                .filter(
                    server_vault_records::record_type
                        .eq(VaultRecordType::AccountCredential.as_str()),
                )
                .filter(server_vault_records::subject_id.eq(&facts.account_id)),
        )
        .execute(connection)
        .map_err(|_| ImportStoreError::MutationFailed)?;
        if removed_account != 1 || removed_vault != 1 {
            return Err(ImportStoreError::MutationConflict);
        }
    }
    Ok(())
}

pub(super) fn rotate_account_credential(
    connection: &mut SqliteConnection,
    current: &CurrentAccountFacts,
    row: &SealedCommitRow,
) -> Result<String, ImportStoreError> {
    let next_credential_revision = current
        .credential_revision
        .checked_add(1)
        .ok_or(ImportStoreError::CredentialRevisionOverflow)?;
    let updated_vault = diesel::update(
        server_vault_records::table
            .filter(server_vault_records::vault_record_id.eq(&current.credential_vault_record_id))
            .filter(
                server_vault_records::record_type.eq(VaultRecordType::AccountCredential.as_str()),
            )
            .filter(server_vault_records::subject_id.eq(&current.account_id)),
    )
    .set((
        server_vault_records::nonce.eq(row.nonce.as_slice()),
        server_vault_records::ciphertext.eq(row.ciphertext.as_slice()),
    ))
    .execute(connection)
    .map_err(|_| ImportStoreError::MutationFailed)?;
    let updated_account = diesel::update(
        accounts::table.filter(accounts::account_id.eq(&current.account_id)),
    )
    .set(
        accounts::credential_revision
            .eq(diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(next_credential_revision)),
    )
    .execute(connection)
    .map_err(|_| ImportStoreError::MutationFailed)?;
    if updated_vault != 1 || updated_account != 1 {
        return Err(ImportStoreError::MutationConflict);
    }
    Ok(current.account_id.clone())
}

pub(super) fn create_account_credential(
    connection: &mut SqliteConnection,
    row: &SealedCommitRow,
) -> Result<String, ImportStoreError> {
    let account_id = Uuid::now_v7();
    let vault_record_id = Uuid::now_v7();
    let inserted_vault = diesel::insert_into(server_vault_records::table)
        .values((
            server_vault_records::vault_record_id.eq(vault_record_id.to_string()),
            server_vault_records::record_type.eq(VaultRecordType::AccountCredential.as_str()),
            server_vault_records::subject_id.eq(account_id.to_string()),
            server_vault_records::nonce.eq(row.nonce.as_slice()),
            server_vault_records::ciphertext.eq(row.ciphertext.as_slice()),
        ))
        .execute(connection)
        .map_err(|_| ImportStoreError::MutationFailed)?;
    let inserted_account = diesel::insert_into(accounts::table)
        .values((
            accounts::account_id.eq(account_id.to_string()),
            accounts::domjudge_username.eq(&row.domjudge_username),
            accounts::credential_vault_record_id.eq(vault_record_id.to_string()),
            accounts::credential_revision
                .eq(diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(1_i64)),
        ))
        .execute(connection)
        .map_err(|_| ImportStoreError::MutationFailed)?;
    if inserted_vault != 1 || inserted_account != 1 {
        return Err(ImportStoreError::MutationConflict);
    }
    Ok(account_id.to_string())
}

pub(super) fn insert_final_mappings(
    connection: &mut SqliteConnection,
    sealed_rows: &[SealedCommitRow],
    current_seats: &BTreeMap<String, CurrentSeatFacts>,
    final_account_ids: &BTreeMap<String, String>,
) -> Result<(), ImportStoreError> {
    for row in sealed_rows {
        let seat_id = current_seats
            .get(&row.seat_code)
            .map_or(row.seat_code.as_str(), |facts| facts.seat_id.as_str());
        let account_id = final_account_ids
            .get(&row.domjudge_username)
            .ok_or(ImportStoreError::InvalidPersistedFacts)?;
        let inserted = diesel::insert_into(account_mappings::table)
            .values((
                account_mappings::seat_id.eq(seat_id),
                account_mappings::account_id.eq(account_id),
            ))
            .execute(connection)
            .map_err(|_| ImportStoreError::MutationFailed)?;
        if inserted != 1 {
            return Err(ImportStoreError::MutationConflict);
        }
    }
    Ok(())
}

pub(super) fn persist_revisions(
    connection: &mut SqliteConnection,
    plan: &CommitPlan,
) -> Result<(), ImportStoreError> {
    if !plan.configuration_revision_advanced && !plan.binding_revision_advanced {
        return Ok(());
    }
    let updated = diesel::update(
        revision_counters::table
            .filter(revision_counters::singleton.eq(Some(1_i32)))
            .filter(
                revision_counters::configuration_revision.eq(diesel::dsl::sql::<Integer>("")
                    .bind::<BigInt, _>(plan.baseline_configuration_revision)),
            )
            .filter(revision_counters::binding_revision.eq(
                diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(plan.baseline_binding_revision),
            )),
    )
    .set(
        (
            revision_counters::configuration_revision
                .eq(diesel::dsl::sql::<Integer>("")
                    .bind::<BigInt, _>(plan.next_configuration_revision)),
            revision_counters::binding_revision
                .eq(diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(plan.next_binding_revision)),
        ),
    )
    .execute(connection)
    .map_err(|_| ImportStoreError::MutationFailed)?;
    if updated != 1 {
        return Err(ImportStoreError::MutationConflict);
    }
    Ok(())
}
