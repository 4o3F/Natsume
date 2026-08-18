use diesel::{
    ExpressionMethods, QueryDsl, RunQueryDsl,
    dsl::sql,
    sql_types::{BigInt, Integer},
};

use crate::{
    application::provisioning::{
        ProvisioningPersistenceError, ProvisioningWindow, ProvisioningWindowState,
    },
    audit::AuditEventId,
    db::{DatabaseError, Transaction, schema::provisioning_window},
};

pub(crate) fn read_window(
    transaction: &mut Transaction<'_>,
) -> Result<ProvisioningWindow, ProvisioningPersistenceError> {
    read_window_persisted(transaction).map_err(ProvisioningPersistenceError::from)
}

fn read_window_persisted(
    transaction: &mut Transaction<'_>,
) -> Result<ProvisioningWindow, ProvisioningStoreError> {
    let (state, revision) = provisioning_window::table
        .select((provisioning_window::state, sql::<BigInt>("revision")))
        .filter(provisioning_window::singleton.eq(Some(1_i32)))
        .first::<(String, i64)>(transaction.connection())
        .map_err(|_| ProvisioningStoreError::ReadFailed)?;
    if revision < 0 {
        return Err(ProvisioningStoreError::InvalidCurrentFacts);
    }
    let state = match state.as_str() {
        "closed" => ProvisioningWindowState::Closed,
        "open" => ProvisioningWindowState::Open,
        _ => return Err(ProvisioningStoreError::InvalidCurrentFacts),
    };
    Ok(ProvisioningWindow { state, revision })
}

pub(crate) fn compare_and_swap_window(
    transaction: &mut Transaction<'_>,
    expected: ProvisioningWindow,
    next: ProvisioningWindow,
    audit_event_id: AuditEventId,
) -> Result<(), ProvisioningPersistenceError> {
    let updated = diesel::update(
        provisioning_window::table
            .filter(provisioning_window::singleton.eq(Some(1_i32)))
            .filter(provisioning_window::state.eq(persisted_state(expected.state)))
            .filter(
                provisioning_window::revision
                    .eq(sql::<Integer>("").bind::<BigInt, _>(expected.revision)),
            ),
    )
    .set((
        provisioning_window::state.eq(persisted_state(next.state)),
        provisioning_window::revision.eq(sql::<Integer>("").bind::<BigInt, _>(next.revision)),
        provisioning_window::last_audit_event_id.eq(Some(audit_event_id.as_text())),
    ))
    .execute(transaction.connection())
    .map_err(|_| ProvisioningStoreError::MutationFailed)?;
    if updated != 1 {
        return Err(ProvisioningStoreError::CompareAndSwapConflict.into());
    }
    Ok(())
}

const fn persisted_state(state: ProvisioningWindowState) -> &'static str {
    match state {
        ProvisioningWindowState::Closed => "closed",
        ProvisioningWindowState::Open => "open",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProvisioningStoreError {
    ReadFailed,
    InvalidCurrentFacts,
    MutationFailed,
    CompareAndSwapConflict,
}

impl From<ProvisioningStoreError> for ProvisioningPersistenceError {
    fn from(error: ProvisioningStoreError) -> Self {
        match error {
            ProvisioningStoreError::ReadFailed | ProvisioningStoreError::MutationFailed => {
                Self::PersistenceFailed
            }
            ProvisioningStoreError::InvalidCurrentFacts
            | ProvisioningStoreError::CompareAndSwapConflict => Self::InvalidPersistedFacts,
        }
    }
}

impl From<DatabaseError> for ProvisioningPersistenceError {
    fn from(error: DatabaseError) -> Self {
        match error {
            DatabaseError::InvalidConfiguration
            | DatabaseError::ConnectionFailed
            | DatabaseError::MigrationFailed
            | DatabaseError::TransactionFailed => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod schema_contract_tests;
