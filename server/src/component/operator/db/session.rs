use diesel::{
    ExpressionMethods, QueryDsl, RunQueryDsl,
    sql_types::{BigInt, Binary, Text},
};
use uuid::Uuid;

use crate::{
    component::operator::OperatorIdentity,
    db::{PersistenceError, Transaction},
    diesel_schema::operator_sessions,
};

/// Atomically admits a session only for the credential revision that was verified.
/// Zero inserted rows is a stale or missing account, not a persistence failure.
pub(in crate::component::operator) fn insert_session_if_current(
    transaction: &mut Transaction<'_>,
    credential_hash: &[u8; 32],
    identity: OperatorIdentity,
    expected_revision: i64,
) -> Result<usize, PersistenceError> {
    diesel::sql_query(
        "INSERT INTO operator_sessions \
         (session_credential_hash, operator_id, expires_at_unix_ms) \
         SELECT ?, operator_id, CAST((unixepoch('subsec') + 57600) * 1000 AS INTEGER) \
         FROM operator_accounts WHERE operator_id = ? AND credential_revision = ?",
    )
    .bind::<Binary, _>(credential_hash.as_slice())
    .bind::<Text, _>(identity.operator_id().to_string())
    .bind::<BigInt, _>(expected_revision)
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::operator) fn delete_session_by_hash(
    transaction: &mut Transaction<'_>,
    credential_hash: &[u8; 32],
) -> Result<usize, PersistenceError> {
    diesel::delete(
        operator_sessions::table
            .filter(operator_sessions::session_credential_hash.eq(credential_hash.as_slice())),
    )
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::operator) fn delete_sessions_by_operator(
    transaction: &mut Transaction<'_>,
    operator_id: Uuid,
) -> Result<usize, PersistenceError> {
    diesel::delete(
        operator_sessions::table.filter(operator_sessions::operator_id.eq(operator_id.to_string())),
    )
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}
