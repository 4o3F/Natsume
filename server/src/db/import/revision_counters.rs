use diesel::{
    ExpressionMethods, QueryDsl, RunQueryDsl,
    sql_types::{BigInt, Integer},
};

use crate::{
    application::contest::ContestPersistenceError,
    db::{Transaction, schema::revision_counters},
};

pub(crate) fn read(
    transaction: &mut Transaction<'_>,
) -> Result<(i64, i64), ContestPersistenceError> {
    let (configuration_revision, binding_revision) = revision_counters::table
        .filter(revision_counters::singleton.eq(Some(1_i32)))
        .select((
            diesel::dsl::sql::<BigInt>("configuration_revision"),
            diesel::dsl::sql::<BigInt>("binding_revision"),
        ))
        .first::<(i64, i64)>(transaction.connection())
        .map_err(|_| ContestPersistenceError::PersistenceFailed)?;
    if configuration_revision < 0 || binding_revision < 0 {
        return Err(ContestPersistenceError::InvalidPersistedFacts);
    }
    Ok((configuration_revision, binding_revision))
}

pub(crate) fn advance(
    transaction: &mut Transaction<'_>,
    baseline_configuration_revision: i64,
    baseline_binding_revision: i64,
    next_configuration_revision: i64,
    next_binding_revision: i64,
) -> Result<usize, ContestPersistenceError> {
    diesel::update(
        revision_counters::table
            .filter(revision_counters::singleton.eq(Some(1_i32)))
            .filter(revision_counters::configuration_revision.eq(
                diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(baseline_configuration_revision),
            ))
            .filter(
                revision_counters::binding_revision
                    .eq(diesel::dsl::sql::<Integer>("")
                        .bind::<BigInt, _>(baseline_binding_revision)),
            ),
    )
    .set((
        revision_counters::configuration_revision
            .eq(diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(next_configuration_revision)),
        revision_counters::binding_revision
            .eq(diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(next_binding_revision)),
    ))
    .execute(transaction.connection())
    .map_err(|_| ContestPersistenceError::PersistenceFailed)
}
