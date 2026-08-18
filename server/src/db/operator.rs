use crate::{application::operator::OperatorError, db::DatabaseError};

mod account;
pub(crate) mod query;
mod session;

pub(crate) use self::{
    account::{any_account_exists, find_account, insert_account, update_password},
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

#[cfg(test)]
impl OperatorStoreError {
    const fn cause(self) -> &'static str {
        match self {
            Self::AccountReadFailed => "operator_account_read_failed",
            Self::AccountInsertFailed => "operator_account_insert_failed",
            Self::AccountUpdateFailed => "operator_account_update_failed",
            Self::AccountUpdateConflict => "operator_account_update_conflict",
            Self::SessionReadFailed => "operator_session_read_failed",
            Self::InvalidPersistedFacts => "operator_invalid_persisted_facts",
            Self::SessionInsertFailed => "operator_session_insert_failed",
            Self::SessionDeleteFailed => "operator_session_delete_failed",
        }
    }
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
pub(crate) mod tests;
