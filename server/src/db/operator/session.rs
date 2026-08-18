use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl, dsl::sql, sql_types::Text};
use uuid::Uuid;

use crate::{
    application::operator::{OperatorError, OperatorIdentity},
    db::{Transaction, schema::operator_sessions},
};

use super::OperatorStoreError;

pub(crate) fn insert_session(
    transaction: &mut Transaction<'_>,
    credential_hash: &[u8; 32],
    identity: OperatorIdentity,
) -> Result<(), OperatorError> {
    diesel::insert_into(operator_sessions::table)
        .values((
            operator_sessions::session_credential_hash.eq(credential_hash.as_slice()),
            operator_sessions::operator_id.eq(identity.operator_id().to_string()),
            operator_sessions::expires_at.eq(sql::<Text>(
                "strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+57600 seconds')",
            )),
        ))
        .execute(transaction.connection())
        .map(|_| ())
        .map_err(|_| OperatorStoreError::SessionInsertFailed)
        .map_err(OperatorError::from)
}

pub(crate) fn delete_session_by_hash(
    transaction: &mut Transaction<'_>,
    credential_hash: &[u8; 32],
) -> Result<usize, OperatorError> {
    diesel::delete(
        operator_sessions::table
            .filter(operator_sessions::session_credential_hash.eq(credential_hash.as_slice())),
    )
    .execute(transaction.connection())
    .map_err(|_| OperatorStoreError::SessionDeleteFailed)
    .map_err(OperatorError::from)
}

pub(crate) fn delete_sessions_by_operator(
    transaction: &mut Transaction<'_>,
    operator_id: Uuid,
) -> Result<usize, OperatorError> {
    diesel::delete(
        operator_sessions::table.filter(operator_sessions::operator_id.eq(operator_id.to_string())),
    )
    .execute(transaction.connection())
    .map_err(|_| OperatorStoreError::SessionDeleteFailed)
    .map_err(OperatorError::from)
}
