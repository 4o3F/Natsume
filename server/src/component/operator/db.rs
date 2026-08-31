use crate::{component::operator::OperatorError, db::DatabaseError};

mod account;
mod query;
mod session;

pub(super) use self::{
    account::{any_account_exists, find_account, insert_account, update_password},
    query::find_session,
    session::{delete_session_by_hash, delete_sessions_by_operator, insert_session},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperatorStoreError {
    AccountReadFailed,
    AccountInsertFailed,
    AccountUpdateFailed,
    AccountUpdateConflict,
    SessionReadFailed,
    InvalidPersistedFacts,
    SessionInsertFailed,
    SessionDeleteFailed,
}

impl From<OperatorStoreError> for OperatorError {
    fn from(source: OperatorStoreError) -> Self {
        match source {
            OperatorStoreError::AccountReadFailed
            | OperatorStoreError::AccountInsertFailed
            | OperatorStoreError::AccountUpdateFailed
            | OperatorStoreError::AccountUpdateConflict
            | OperatorStoreError::SessionReadFailed
            | OperatorStoreError::InvalidPersistedFacts
            | OperatorStoreError::SessionInsertFailed
            | OperatorStoreError::SessionDeleteFailed => Self::PersistenceFailed,
        }
    }
}

impl From<DatabaseError> for OperatorError {
    fn from(source: DatabaseError) -> Self {
        match source {
            DatabaseError::InvalidConfiguration
            | DatabaseError::ConnectionFailed
            | DatabaseError::MigrationFailed
            | DatabaseError::TransactionFailed => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
pub(super) mod tests;
