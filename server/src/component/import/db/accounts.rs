use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};
use uuid::Uuid;

use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::accounts,
};

use super::super::baseline::BaselineAccount;

pub(in crate::component::import) fn insert(
    transaction: &mut Transaction<'_>,
    account_id: Uuid,
    domjudge_username: &str,
) -> Result<usize, PersistenceError> {
    diesel::insert_into(accounts::table)
        .values((
            accounts::account_id.eq(account_id.to_string()),
            accounts::domjudge_username.eq(domjudge_username),
            accounts::credential_revision.eq(1_i64),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::import) fn delete_exact(
    transaction: &mut Transaction<'_>,
    account: &BaselineAccount,
) -> Result<usize, PersistenceError> {
    diesel::delete(
        accounts::table
            .filter(accounts::account_id.eq(account.account_id()))
            .filter(accounts::domjudge_username.eq(account.domjudge_username()))
            .filter(accounts::credential_revision.eq(account.credential_revision())),
    )
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::import) fn advance_credential_revision(
    transaction: &mut Transaction<'_>,
    account: &BaselineAccount,
    next: i64,
) -> Result<usize, PersistenceError> {
    diesel::update(
        accounts::table
            .filter(accounts::account_id.eq(account.account_id()))
            .filter(accounts::domjudge_username.eq(account.domjudge_username()))
            .filter(accounts::credential_revision.eq(account.credential_revision())),
    )
    .set(accounts::credential_revision.eq(next))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}
