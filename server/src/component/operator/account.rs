use uuid::Uuid;

use crate::db::Database;

use super::{OperatorError, OperatorIdentity, OperatorRole};

pub(super) struct AccountFacts {
    pub(super) identity: OperatorIdentity,
    pub(super) password_hash: String,
}

/// Creates the only bootstrap administrator.
pub(super) async fn create_first_admin(
    database: &Database,
    login_name: &str,
    password_hash: &str,
) -> Result<Uuid, OperatorError> {
    let operator_id = Uuid::now_v7();
    let login_name = login_name.to_owned();
    let password_hash = password_hash.to_owned();
    database
        .write(move |transaction| {
            if crate::component::operator::db::any_account_exists(transaction)? {
                return Err(OperatorError::PersistenceFailed);
            }
            crate::component::operator::db::insert_account(
                transaction,
                operator_id,
                &login_name,
                OperatorRole::Admin,
                &password_hash,
            )?;
            Ok(operator_id)
        })
        .await
}

/// Replaces one operator password and removes exactly that operator's sessions.
pub(super) async fn reset_operator_password(
    database: &Database,
    login_name: &str,
    password_hash: &str,
) -> Result<(), OperatorError> {
    let login_name = login_name.to_owned();
    let password_hash = password_hash.to_owned();
    let result = database
        .write(move |transaction| {
            let account = crate::component::operator::db::find_account(transaction, &login_name)?
                .ok_or(OperatorError::PersistenceFailed)?;
            crate::component::operator::db::update_password(
                transaction,
                account.identity.operator_id(),
                &password_hash,
            )?;
            crate::component::operator::db::delete_sessions_by_operator(
                transaction,
                account.identity.operator_id(),
            )?;
            Ok(())
        })
        .await;
    if result.is_err() {
        tracing::warn!("operator password reset failed");
    }
    result
}
