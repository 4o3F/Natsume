use uuid::Uuid;

use crate::{
    component::operator::{OperatorError, OperatorRole},
    db::{Database, PersistenceError, TransactionError},
};

#[derive(diesel::QueryableByName)]
struct IntegerValue {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    value: i64,
}

pub(in crate::component::operator) async fn test_now(
    database: &Database,
) -> Result<i64, OperatorError> {
    database
        .read(|transaction| {
            use diesel::RunQueryDsl as _;
            diesel::sql_query("SELECT CAST(unixepoch('subsec') * 1000 AS INTEGER) AS value")
                .get_result::<IntegerValue>(transaction.connection())
                .map(|row| row.value)
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(OperatorError::from)
}

pub(in crate::component::operator) async fn test_sessions_have_expected_ttl(
    database: &Database,
    before: &i64,
    after: &i64,
) -> Result<bool, OperatorError> {
    let lower = before.saturating_add(57_600_000);
    let upper = after.saturating_add(57_600_000);
    database
        .read(move |transaction| -> Result<_, PersistenceError> {
            use diesel::RunQueryDsl as _;
            let row = diesel::sql_query(
                "SELECT COUNT(*) AS value FROM operator_sessions \
                 WHERE expires_at_unix_ms BETWEEN ? AND ?",
            )
            .bind::<diesel::sql_types::BigInt, _>(lower)
            .bind::<diesel::sql_types::BigInt, _>(upper)
            .get_result::<IntegerValue>(transaction.connection())
            .map_err(|_| PersistenceError::OperationFailed)?;
            Ok(row.value == 2)
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(OperatorError::from)
}

pub(in crate::component::operator) async fn test_session_hashes(
    database: &Database,
) -> Result<Vec<Vec<u8>>, OperatorError> {
    database
        .read(|transaction| {
            use diesel::{QueryDsl as _, RunQueryDsl as _};
            crate::diesel_schema::operator_sessions::table
                .select(crate::diesel_schema::operator_sessions::session_credential_hash)
                .load::<Vec<u8>>(transaction.connection())
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(OperatorError::from)
}

pub(in crate::component::operator) async fn test_session_count(
    database: &Database,
) -> Result<i64, OperatorError> {
    database
        .read(|transaction| {
            use diesel::RunQueryDsl as _;
            diesel::sql_query("SELECT COUNT(*) AS value FROM operator_sessions")
                .get_result::<IntegerValue>(transaction.connection())
                .map(|row| row.value)
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(OperatorError::from)
}

pub(in crate::component::operator) async fn test_expire_all_sessions(
    database: &Database,
) -> Result<(), OperatorError> {
    database
        .write(|transaction| {
            use diesel::RunQueryDsl as _;
            diesel::sql_query("UPDATE operator_sessions SET expires_at_unix_ms = 0")
                .execute(transaction.connection())
                .map(|_| ())
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(OperatorError::from)
}

pub(in crate::component::operator) async fn test_insert_account(
    database: &Database,
    login_name: &str,
    role: OperatorRole,
    password_hash: &str,
) -> Result<Uuid, OperatorError> {
    let operator_id = Uuid::now_v7();
    let login_name = login_name.to_owned();
    let password_hash = password_hash.to_owned();
    database
        .write(move |transaction| -> Result<_, PersistenceError> {
            super::insert_account(transaction, operator_id, &login_name, role, &password_hash)?;
            Ok(operator_id)
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(OperatorError::from)
}

pub(in crate::component::operator) async fn test_account_credentials(
    database: &Database,
    login_name: &str,
) -> Result<(String, i64), OperatorError> {
    let login_name = login_name.to_owned();
    database
        .read(move |transaction| {
            use crate::diesel_schema::operator_accounts;
            use diesel::{ExpressionMethods as _, QueryDsl as _, RunQueryDsl as _};

            operator_accounts::table
                .filter(operator_accounts::username.eq(login_name))
                .select((
                    operator_accounts::password_hash,
                    operator_accounts::credential_revision,
                ))
                .first(transaction.connection())
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(OperatorError::from)
}

pub(in crate::component::operator) async fn test_set_credential_revision(
    database: &Database,
    login_name: &str,
    revision: i64,
) -> Result<(), OperatorError> {
    let login_name = login_name.to_owned();
    database
        .write(move |transaction| {
            use crate::diesel_schema::operator_accounts;
            use diesel::{ExpressionMethods as _, QueryDsl as _, RunQueryDsl as _};

            diesel::update(
                operator_accounts::table.filter(operator_accounts::username.eq(login_name)),
            )
            .set(operator_accounts::credential_revision.eq(revision))
            .execute(transaction.connection())
            .map(|_| ())
            .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(OperatorError::from)
}

pub(in crate::component::operator) async fn test_reject_session_deletion(
    database: &Database,
) -> Result<(), OperatorError> {
    database
        .write(|transaction| {
            use diesel::connection::SimpleConnection as _;

            transaction
                .connection()
                .batch_execute(
                    "CREATE TRIGGER reject_session_deletion BEFORE DELETE ON operator_sessions \
                 BEGIN SELECT RAISE(ABORT, 'test session deletion failure'); END;",
                )
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(OperatorError::from)
}
