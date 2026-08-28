use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};

use crate::{
    component::{
        contest::{CurrentAccountProjection, NewAccountFacts},
        import::ImportError,
    },
    db::Transaction,
    diesel_schema::accounts,
};

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    account: &NewAccountFacts,
) -> Result<usize, ImportError> {
    diesel::insert_into(accounts::table)
        .values((
            accounts::account_id.eq(account.account_id().to_string()),
            accounts::domjudge_username.eq(account.domjudge_username()),
            accounts::credential_revision.eq(1_i64),
        ))
        .execute(transaction.connection())
        .map_err(|_| ImportError::PersistenceFailure)
}

pub(crate) fn delete_exact(
    transaction: &mut Transaction<'_>,
    account: &CurrentAccountProjection,
) -> Result<usize, ImportError> {
    diesel::delete(
        accounts::table
            .filter(accounts::account_id.eq(account.account_id()))
            .filter(accounts::domjudge_username.eq(account.domjudge_username()))
            .filter(accounts::credential_revision.eq(account.credential_revision())),
    )
    .execute(transaction.connection())
    .map_err(|_| ImportError::PersistenceFailure)
}

pub(crate) fn advance_credential_revision(
    transaction: &mut Transaction<'_>,
    account: &CurrentAccountProjection,
    next: i64,
) -> Result<usize, ImportError> {
    diesel::update(
        accounts::table
            .filter(accounts::account_id.eq(account.account_id()))
            .filter(accounts::domjudge_username.eq(account.domjudge_username()))
            .filter(accounts::credential_revision.eq(account.credential_revision())),
    )
    .set(accounts::credential_revision.eq(next))
    .execute(transaction.connection())
    .map_err(|_| ImportError::PersistenceFailure)
}
