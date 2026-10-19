use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl};

use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::target_submission_receipts as receipts,
};

pub(super) fn find(
    transaction: &mut Transaction<'_>,
    id: &str,
) -> Result<Option<(String, String, String)>, PersistenceError> {
    receipts::table
        .filter(receipts::operation_id.eq(id))
        .select((
            receipts::operator_id,
            receipts::request_json,
            receipts::results_json,
        ))
        .first(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(super) fn insert(
    transaction: &mut Transaction<'_>,
    id: &str,
    owner: &str,
    request: &str,
    results: &str,
) -> Result<(), PersistenceError> {
    let changed = diesel::insert_into(receipts::table)
        .values((
            receipts::operation_id.eq(id),
            receipts::operator_id.eq(owner),
            receipts::request_json.eq(request),
            receipts::results_json.eq(results),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?;
    if changed != 1 {
        return Err(PersistenceError::InvalidPersistedData);
    }
    Ok(())
}
