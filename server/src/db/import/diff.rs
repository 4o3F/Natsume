use std::collections::{BTreeMap, BTreeSet};

use diesel::{
    ExpressionMethods, JoinOnDsl, NullableExpressionMethods, QueryDsl, RunQueryDsl,
    sql_types::BigInt, sqlite::SqliteConnection,
};

use crate::{
    application::import::{
        CandidateRowFacts, ImportBindingImpact, ImportMappingChange, RedactedImportPreview,
    },
    db::schema::{account_mappings, accounts, device_bindings, revision_counters, seats},
};

use super::ImportStoreError;

pub(super) fn read_revision_counters(
    connection: &mut SqliteConnection,
) -> Result<(i64, i64), ImportStoreError> {
    let (configuration_revision, binding_revision) = revision_counters::table
        .filter(revision_counters::singleton.eq(Some(1_i32)))
        .select((
            diesel::dsl::sql::<BigInt>("configuration_revision"),
            diesel::dsl::sql::<BigInt>("binding_revision"),
        ))
        .first::<(i64, i64)>(connection)
        .map_err(|_| ImportStoreError::RevisionsReadFailed)?;
    if configuration_revision < 0 || binding_revision < 0 {
        return Err(ImportStoreError::InvalidPersistedFacts);
    }
    Ok((configuration_revision, binding_revision))
}

pub(super) struct CurrentSeatFacts {
    pub(super) seat_id: String,
    pub(super) current_domjudge_username: Option<String>,
    pub(super) device_id: Option<String>,
}

pub(super) fn read_current_seats(
    connection: &mut SqliteConnection,
) -> Result<BTreeMap<String, CurrentSeatFacts>, ImportStoreError> {
    let rows = seats::table
        .left_join(account_mappings::table.on(account_mappings::seat_id.eq(seats::seat_id)))
        .left_join(accounts::table.on(account_mappings::account_id.eq(accounts::account_id)))
        .left_join(device_bindings::table.on(device_bindings::seat_id.eq(seats::seat_id)))
        .select((
            seats::seat_id,
            seats::seat_code,
            accounts::domjudge_username.nullable(),
            device_bindings::device_pk.nullable(),
        ))
        .order(seats::seat_code)
        .load::<(String, String, Option<String>, Option<String>)>(connection)
        .map_err(|_| ImportStoreError::CurrentFactsReadFailed)?;

    let mut current = BTreeMap::new();
    for (seat_id, seat_code, current_domjudge_username, device_id) in rows {
        if current
            .insert(
                seat_code,
                CurrentSeatFacts {
                    seat_id,
                    current_domjudge_username,
                    device_id,
                },
            )
            .is_some()
        {
            return Err(ImportStoreError::InvalidPersistedFacts);
        }
    }
    Ok(current)
}

pub(super) fn compute_diff(
    current: &BTreeMap<String, CurrentSeatFacts>,
    candidate_rows: &[CandidateRowFacts],
) -> Result<RedactedImportPreview, ImportStoreError> {
    let mut candidate = BTreeMap::new();
    let mut candidate_accounts = BTreeSet::new();
    for row in candidate_rows {
        if candidate
            .insert(row.seat_code.as_str(), row.domjudge_username.as_str())
            .is_some()
            || !candidate_accounts.insert(row.domjudge_username.as_str())
        {
            return Err(ImportStoreError::InvalidCandidateFacts);
        }
    }
    if candidate.is_empty() {
        return Err(ImportStoreError::InvalidCandidateFacts);
    }

    let seats_added = candidate
        .keys()
        .filter(|seat_code| !current.contains_key(**seat_code))
        .map(|seat_code| (*seat_code).to_owned())
        .collect();
    let mut seats_removed = Vec::new();
    let mut mappings_changed = Vec::new();
    let mut unchanged_count = 0;
    let mut binding_impacts = Vec::new();

    for (seat_code, facts) in current {
        let Some(candidate_username) = candidate.get(seat_code.as_str()) else {
            seats_removed.push(seat_code.clone());
            if let Some(device_id) = &facts.device_id {
                binding_impacts.push(ImportBindingImpact::new(
                    seat_code.clone(),
                    device_id.clone(),
                ));
            }
            continue;
        };
        if facts.current_domjudge_username.as_deref() == Some(*candidate_username) {
            unchanged_count += 1;
        } else {
            mappings_changed.push(ImportMappingChange::new(
                seat_code.clone(),
                facts.current_domjudge_username.clone(),
                (*candidate_username).to_owned(),
            ));
        }
    }

    Ok(RedactedImportPreview::new(
        seats_added,
        seats_removed,
        mappings_changed,
        unchanged_count,
        candidate_accounts.len(),
        binding_impacts,
    ))
}
