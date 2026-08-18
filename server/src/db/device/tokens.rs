use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl, upsert::excluded};
use uuid::Uuid;

use crate::{
    application::device::DevicePersistenceError,
    db::{Transaction, schema::device_tokens},
};

pub(crate) fn upsert(
    transaction: &mut Transaction<'_>,
    device_id: Uuid,
    enrollment_request_id: Uuid,
    token_hash: [u8; 32],
) -> Result<(), DevicePersistenceError> {
    let affected = diesel::insert_into(device_tokens::table)
        .values((
            device_tokens::device_pk.eq(device_id.to_string()),
            device_tokens::enrollment_request_id.eq(enrollment_request_id.to_string()),
            device_tokens::token_hash.eq(token_hash.as_slice()),
        ))
        .on_conflict(device_tokens::device_pk)
        .do_update()
        .set((
            device_tokens::enrollment_request_id.eq(excluded(device_tokens::enrollment_request_id)),
            device_tokens::token_hash.eq(excluded(device_tokens::token_hash)),
        ))
        .execute(transaction.connection())
        .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    if affected != 1 {
        return Err(DevicePersistenceError::PersistenceFailed);
    }
    Ok(())
}

pub(crate) fn delete(
    transaction: &mut Transaction<'_>,
    device_id: &str,
) -> Result<i64, DevicePersistenceError> {
    let removed =
        diesel::delete(device_tokens::table.filter(device_tokens::device_pk.eq(device_id)))
            .execute(transaction.connection())
            .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    i64::try_from(removed).map_err(|_| DevicePersistenceError::InvalidPersistedFacts)
}
