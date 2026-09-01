use uuid::Uuid;

use crate::{
    component::operator::{OperatorError, OperatorRole},
    db::Database,
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
                .map_err(|_| OperatorError::PersistenceFailed)
        })
        .await
}

pub(in crate::component::operator) async fn test_sessions_have_expected_ttl(
    database: &Database,
    before: &i64,
    after: &i64,
) -> Result<bool, OperatorError> {
    let lower = before.saturating_add(57_600_000);
    let upper = after.saturating_add(57_600_000);
    database
        .read(move |transaction| {
            use diesel::RunQueryDsl as _;
            let row = diesel::sql_query(
                "SELECT COUNT(*) AS value FROM operator_sessions \
                 WHERE expires_at_unix_ms BETWEEN ? AND ?",
            )
            .bind::<diesel::sql_types::BigInt, _>(lower)
            .bind::<diesel::sql_types::BigInt, _>(upper)
            .get_result::<IntegerValue>(transaction.connection())
            .map_err(|_| OperatorError::PersistenceFailed)?;
            Ok(row.value == 2)
        })
        .await
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
                .map_err(|_| OperatorError::PersistenceFailed)
        })
        .await
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
                .map_err(|_| OperatorError::PersistenceFailed)
        })
        .await
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
                .map_err(|_| OperatorError::PersistenceFailed)
        })
        .await
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
        .write(move |transaction| {
            super::insert_account(transaction, operator_id, &login_name, role, &password_hash)?;
            Ok(operator_id)
        })
        .await
}
