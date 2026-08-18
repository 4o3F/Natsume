use diesel::{
    ExpressionMethods, QueryDsl, RunQueryDsl,
    dsl::sql,
    sql_types::{BigInt, Integer},
};

use crate::{
    application::contest::{
        AccountFacts, ContestPersistenceError, CurrentAccountProjection, NewAccountFacts,
    },
    db::{Transaction, schema::accounts},
};

pub(crate) fn list(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<AccountFacts>, ContestPersistenceError> {
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
        .map_err(|_| ContestPersistenceError::PersistenceFailed)
}

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    account: &NewAccountFacts,
) -> Result<usize, ContestPersistenceError> {
    diesel::insert_into(accounts::table)
        .values((
            accounts::account_id.eq(account.account_id().to_string()),
            accounts::domjudge_username.eq(account.domjudge_username()),
            accounts::credential_vault_record_id
                .eq(account.credential_vault_record_id().to_string()),
            accounts::credential_revision
                .eq(diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(1_i64)),
        ))
        .execute(transaction.connection())
        .map_err(|_| ContestPersistenceError::PersistenceFailed)
}

pub(crate) fn delete_exact(
    transaction: &mut Transaction<'_>,
    account: &CurrentAccountProjection,
) -> Result<usize, ContestPersistenceError> {
    diesel::delete(
        accounts::table
            .filter(accounts::account_id.eq(account.account_id()))
            .filter(accounts::domjudge_username.eq(account.domjudge_username()))
            .filter(accounts::credential_vault_record_id.eq(account.credential_vault_record_id()))
            .filter(accounts::credential_revision.eq(
                diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(account.credential_revision()),
            )),
    )
    .execute(transaction.connection())
    .map_err(|_| ContestPersistenceError::PersistenceFailed)
}

pub(crate) fn advance_credential_revision(
    transaction: &mut Transaction<'_>,
    account: &CurrentAccountProjection,
    next_credential_revision: i64,
) -> Result<usize, ContestPersistenceError> {
    diesel::update(
        accounts::table
            .filter(accounts::account_id.eq(account.account_id()))
            .filter(accounts::domjudge_username.eq(account.domjudge_username()))
            .filter(accounts::credential_vault_record_id.eq(account.credential_vault_record_id()))
            .filter(accounts::credential_revision.eq(
                diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(account.credential_revision()),
            )),
    )
    .set(
        accounts::credential_revision
            .eq(diesel::dsl::sql::<Integer>("").bind::<BigInt, _>(next_credential_revision)),
    )
    .execute(transaction.connection())
    .map_err(|_| ContestPersistenceError::PersistenceFailed)
}
