use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl,
    dsl::{exists, select},
};
use uuid::Uuid;

use crate::{
    component::operator::{AccountFacts, OperatorIdentity, OperatorRole},
    db::{PersistenceError, Transaction},
    diesel_schema::operator_accounts,
};

pub(in crate::component::operator) fn find_account(
    transaction: &mut Transaction<'_>,
    login_name: &str,
) -> Result<Option<AccountFacts>, PersistenceError> {
    let row = operator_accounts::table
        .filter(operator_accounts::username.eq(login_name))
        .select((
            operator_accounts::operator_id,
            operator_accounts::role,
            operator_accounts::password_hash,
        ))
        .first::<(String, String, String)>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?;
    row.map(|(operator_id, role, password_hash)| {
        let identity = OperatorIdentity::from_persisted(&operator_id, &role)
            .map_err(|_| PersistenceError::InvalidPersistedData)?;
        Ok(AccountFacts {
            identity,
            password_hash,
        })
    })
    .transpose()
}

pub(in crate::component::operator) fn any_account_exists(
    transaction: &mut Transaction<'_>,
) -> Result<bool, PersistenceError> {
    select(exists(
        operator_accounts::table.select(operator_accounts::operator_id),
    ))
    .get_result::<bool>(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::operator) fn insert_account(
    transaction: &mut Transaction<'_>,
    operator_id: Uuid,
    login_name: &str,
    role: OperatorRole,
    password_hash: &str,
) -> Result<(), PersistenceError> {
    diesel::insert_into(operator_accounts::table)
        .values((
            operator_accounts::operator_id.eq(operator_id.to_string()),
            operator_accounts::username.eq(login_name),
            operator_accounts::role.eq(role.as_persisted()),
            operator_accounts::password_hash.eq(password_hash),
        ))
        .execute(transaction.connection())
        .map(|_| ())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::operator) fn update_password(
    transaction: &mut Transaction<'_>,
    operator_id: Uuid,
    password_hash: &str,
) -> Result<(), PersistenceError> {
    let updated = diesel::update(
        operator_accounts::table.filter(operator_accounts::operator_id.eq(operator_id.to_string())),
    )
    .set(operator_accounts::password_hash.eq(password_hash))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)?;
    if updated != 1 {
        return Err(PersistenceError::InvalidPersistedData);
    }
    Ok(())
}
