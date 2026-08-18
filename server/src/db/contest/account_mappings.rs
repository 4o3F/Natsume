use diesel::{ExpressionMethods, RunQueryDsl};

use crate::{
    application::contest::ContestPersistenceError,
    db::{Transaction, schema::account_mappings},
};

pub(crate) fn delete_all(
    transaction: &mut Transaction<'_>,
) -> Result<usize, ContestPersistenceError> {
    diesel::delete(account_mappings::table)
        .execute(transaction.connection())
        .map_err(|_| ContestPersistenceError::PersistenceFailed)
}

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    seat_id: &str,
    account_id: &str,
) -> Result<usize, ContestPersistenceError> {
    diesel::insert_into(account_mappings::table)
        .values((
            account_mappings::seat_id.eq(seat_id),
            account_mappings::account_id.eq(account_id),
        ))
        .execute(transaction.connection())
        .map_err(|_| ContestPersistenceError::PersistenceFailed)
}
