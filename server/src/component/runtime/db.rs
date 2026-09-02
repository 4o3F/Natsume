use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};

use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::runtime_config,
};

pub(in crate::component::runtime) fn read_all(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<(i32, String)>, PersistenceError> {
    runtime_config::table
        .select((runtime_config::singleton, runtime_config::domjudge_origin))
        .load(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::runtime) fn replace(
    transaction: &mut Transaction<'_>,
    domjudge_origin: &str,
) -> Result<(), PersistenceError> {
    diesel::replace_into(runtime_config::table)
        .values((
            runtime_config::singleton.eq(1),
            runtime_config::domjudge_origin.eq(domjudge_origin),
        ))
        .execute(transaction.connection())
        .map(|_| ())
        .map_err(|_| PersistenceError::OperationFailed)
}
