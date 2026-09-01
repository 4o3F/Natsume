use diesel::{QueryDsl, RunQueryDsl, dsl::sql, sql_types::BigInt};

use crate::{
    component::contest::AccountFacts,
    db::{PersistenceError, Transaction},
    diesel_schema::accounts,
};

pub(in crate::component::contest) fn list(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<AccountFacts>, PersistenceError> {
    accounts::table
        .select((
            accounts::account_id,
            accounts::domjudge_username,
            sql::<BigInt>("credential_revision"),
        ))
        .order(accounts::account_id)
        .load::<(String, String, i64)>(transaction.connection())
        .map(|rows| {
            rows.into_iter()
                .map(|(account_id, domjudge_username, credential_revision)| {
                    AccountFacts::new(account_id, domjudge_username, credential_revision)
                })
                .collect()
        })
        .map_err(|_| PersistenceError::OperationFailed)
}
