use diesel::{ExpressionMethods, RunQueryDsl};

use crate::{component::import::ImportError, db::Transaction, diesel_schema::account_mappings};

pub(in crate::component::import) fn delete_all(
    transaction: &mut Transaction<'_>,
) -> Result<usize, ImportError> {
    diesel::delete(account_mappings::table)
        .execute(transaction.connection())
        .map_err(|_| ImportError::PersistenceFailure)
}

pub(in crate::component::import) fn insert(
    transaction: &mut Transaction<'_>,
    seat_id: &str,
    account_id: &str,
) -> Result<usize, ImportError> {
    diesel::insert_into(account_mappings::table)
        .values((
            account_mappings::seat_id.eq(seat_id),
            account_mappings::account_id.eq(account_id),
        ))
        .execute(transaction.connection())
        .map_err(|_| ImportError::PersistenceFailure)
}
