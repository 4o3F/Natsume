use crate::db::PersistenceError;
mod account;
pub(super) mod links;
pub(super) mod management;
mod query;
mod session;

pub(super) use self::{
    account::{any_account_exists, find_account, insert_account, update_password},
    query::find_session,
    session::{delete_session_by_hash, delete_sessions_by_operator, insert_session_if_current},
};

#[cfg(test)]
pub(super) mod tests;

fn persisted_identity_error(error: super::OperatorError) -> PersistenceError {
    match error {
        super::OperatorError::InvalidPersistedIdentity { source } => PersistenceError::Uuid {
            field: "operator identity",
            source,
        },
        super::OperatorError::InvalidPersistedRole
        | super::OperatorError::AuthenticationFailed
        | super::OperatorError::SignInBusy
        | super::OperatorError::CredentialsTooLong
        | super::OperatorError::SessionAuthenticationFailed
        | super::OperatorError::SessionCleanup { .. }
        | super::OperatorError::AuthorizationDenied
        | super::OperatorError::PersistenceFailed { .. }
        | super::OperatorError::PasswordTaskFailed { .. }
        | super::OperatorError::PasswordVerificationFailed
        | super::OperatorError::PasswordVerification { .. }
        | super::OperatorError::InvalidSessionCredential
        | super::OperatorError::EmptyLoginName
        | super::OperatorError::InvalidNewLoginName
        | super::OperatorError::InvalidNewPassword
        | super::OperatorError::PasswordMismatch
        | super::OperatorError::EntropyUnavailable { .. }
        | super::OperatorError::InvalidHashingParameters { .. }
        | super::OperatorError::SaltEncodingFailed { .. }
        | super::OperatorError::PasswordHashingFailed { .. }
        | super::OperatorError::OperatorNotFound
        | super::OperatorError::LoginNameConflict
        | super::OperatorError::LastAdmin
        | super::OperatorError::LinkUnavailable
        | super::OperatorError::CredentialChanged => PersistenceError::invalid("operator role"),
    }
}
