use diesel::{QueryDsl, RunQueryDsl};

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
