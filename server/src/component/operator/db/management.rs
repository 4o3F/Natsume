use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl};
use uuid::Uuid;

use super::super::{AccountFacts, OperatorError, OperatorIdentity, OperatorRole, OperatorSummary};
use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::operator_accounts,
};

pub(in crate::component::operator) fn list_accounts(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<OperatorSummary>, PersistenceError> {
    operator_accounts::table
        .order((
            operator_accounts::username.asc(),
            operator_accounts::operator_id.asc(),
        ))
        .select((
            operator_accounts::operator_id,
            operator_accounts::username,
            operator_accounts::role,
        ))
        .load::<(String, String, String)>(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?
        .into_iter()
        .map(|(id, username, role)| {
            let identity = OperatorIdentity::from_persisted(&id, &role)
                .map_err(|_| PersistenceError::InvalidPersistedData)?;
            Ok(OperatorSummary {
                operator_id: identity.operator_id(),
                username,
                role: identity.role,
            })
        })
        .collect()
}

pub(in crate::component::operator) fn find_account_by_id(
    transaction: &mut Transaction<'_>,
    id: Uuid,
) -> Result<Option<AccountFacts>, PersistenceError> {
    let name = operator_accounts::table
        .filter(operator_accounts::operator_id.eq(id.to_string()))
        .select(operator_accounts::username)
        .first::<String>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?;
    name.map(|name| super::find_account(transaction, &name))
        .transpose()
        .map(Option::flatten)
}

pub(in crate::component::operator) fn account_name(
    transaction: &mut Transaction<'_>,
    id: Uuid,
) -> Result<String, OperatorError> {
    operator_accounts::table
        .filter(operator_accounts::operator_id.eq(id.to_string()))
        .select(operator_accounts::username)
        .first(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
        .ok_or(OperatorError::OperatorNotFound)
}

pub(in crate::component::operator) fn protect_last_admin(
    transaction: &mut Transaction<'_>,
    id: Uuid,
) -> Result<(), OperatorError> {
    let account = find_account_by_id(transaction, id)?.ok_or(OperatorError::OperatorNotFound)?;
    if account.identity.role == OperatorRole::Admin {
        let admins = operator_accounts::table
            .filter(operator_accounts::role.eq("admin"))
            .count()
            .get_result::<i64>(transaction.connection())
            .map_err(|_| PersistenceError::OperationFailed)?;
        if admins <= 1 {
            return Err(OperatorError::LastAdmin);
        }
    }
    Ok(())
}

pub(in crate::component::operator) fn update_role(
    transaction: &mut Transaction<'_>,
    id: Uuid,
    role: OperatorRole,
) -> Result<(), PersistenceError> {
    let changed = diesel::update(
        operator_accounts::table.filter(operator_accounts::operator_id.eq(id.to_string())),
    )
    .set(operator_accounts::role.eq(role.as_persisted()))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)?;
    if changed != 1 {
        return Err(PersistenceError::InvalidPersistedData);
    }
    Ok(())
}

pub(in crate::component::operator) fn delete_account(
    transaction: &mut Transaction<'_>,
    id: Uuid,
) -> Result<(), PersistenceError> {
    let deleted = diesel::delete(
        operator_accounts::table.filter(operator_accounts::operator_id.eq(id.to_string())),
    )
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)?;
    if deleted != 1 {
        return Err(PersistenceError::InvalidPersistedData);
    }
    Ok(())
}
