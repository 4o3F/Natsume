use uuid::Uuid;

use crate::db::{Database, PersistenceError, Transaction, TransactionError};

use super::{OperatorError, OperatorIdentity, OperatorRole};

pub(super) struct AccountFacts {
    pub(super) identity: OperatorIdentity,
    pub(super) password_hash: String,
    pub(super) credential_revision: i64,
}

/// Creates the only bootstrap administrator.
pub(super) fn create_first_admin(
    transaction: &mut Transaction<'_>,
    login_name: &str,
    password_hash: &str,
) -> Result<Uuid, OperatorError> {
    let operator_id = Uuid::now_v7();
    if crate::component::operator::db::any_account_exists(transaction)? {
        return Err(OperatorError::from(PersistenceError::InvalidPersistedData));
    }
    crate::component::operator::db::insert_account(
        transaction,
        operator_id,
        login_name,
        OperatorRole::Admin,
        password_hash,
    )?;
    Ok(operator_id)
}

/// Advances one operator's credential revision with its password replacement and
/// session deletion, fencing both pending sign-ins and already-issued sessions.
pub(super) async fn reset_operator_password(
    database: &Database,
    login_name: &str,
    password_hash: &str,
) -> Result<(), OperatorError> {
    let login_name = login_name.to_owned();
    let password_hash = password_hash.to_owned();
    let result = database
        .write(move |transaction| -> Result<_, OperatorError> {
            let account = crate::component::operator::db::find_account(transaction, &login_name)?
                .ok_or(PersistenceError::InvalidPersistedData)?;
            replace_password(transaction, &account, &password_hash)
        })
        .await
        .map_err(TransactionError::into_error);
    if result.is_err() {
        tracing::warn!("operator password reset failed");
    }
    result
}

/// All password replacement paths advance the fence and revoke sessions and
/// target recovery grants within the same transaction, including TTY recovery.
pub(super) fn replace_password(
    transaction: &mut Transaction<'_>,
    account: &AccountFacts,
    password_hash: &str,
) -> Result<(), OperatorError> {
    let next_revision = account
        .credential_revision
        .checked_add(1)
        .ok_or(OperatorError::PersistenceFailed)?;
    let id = account.identity.operator_id();
    super::db::update_password(transaction, id, password_hash, next_revision)?;
    super::db::delete_sessions_by_operator(transaction, id)?;
    super::db::links::delete_target_resets(transaction, id)?;
    Ok(())
}
