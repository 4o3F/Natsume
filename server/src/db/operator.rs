use snafu::Snafu;

use crate::application::operator::OperatorError;

mod bootstrap;
mod password_reset;
mod session;

pub(crate) use self::bootstrap::create_first_admin;
pub(crate) use self::password_reset::reset_operator_password;
pub(crate) use self::session::{create_session, read_account, read_session, terminate_session};

#[cfg(test)]
use self::{
    bootstrap::{CreateFirstAdminError, create_first_admin_with_ids},
    password_reset::{ResetOperatorPasswordError, reset_operator_password_with_ids},
    session::{create_session_with_audit_id, terminate_session_with_audit_id},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
#[snafu(module)]
pub(super) enum OperatorStoreError {
    #[snafu(display("the operator database connection could not be acquired"))]
    AcquireFailed,
    #[snafu(display("the operator transaction failed"))]
    TransactionFailed,
    #[snafu(display("the operator account could not be read"))]
    AccountReadFailed,
    #[snafu(display("the operator session could not be read"))]
    SessionReadFailed,
    #[snafu(display("the expired operator session could not be cleaned up"))]
    ExpiredSessionCleanupFailed,
    #[snafu(display("persisted operator facts were invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("the operator session could not be inserted"))]
    SessionInsertFailed,
    #[snafu(display("the operator session could not be deleted"))]
    SessionDeleteFailed,
    #[snafu(display("the operator session changed concurrently"))]
    SessionDeleteConflict,
    #[snafu(display("the operator-session audit could not be inserted"))]
    AuditInsertFailed,
}

impl From<diesel::result::Error> for OperatorStoreError {
    /// Transaction control is the only stage that reports a raw Diesel error,
    /// and the source is discarded so no SQL text can reach a log or response.
    fn from(_source: diesel::result::Error) -> Self {
        Self::TransactionFailed
    }
}

impl From<OperatorStoreError> for OperatorError {
    /// The store vocabulary never leaves this module, so every entry point
    /// collapses it here.
    ///
    /// `ExpiredSessionCleanupFailed` keeps the expired classification the read
    /// already observed before the lazy cleanup failed, because that is the safer
    /// public one: reporting the cleanup failure instead would make an expired
    /// credential distinguishable from an unknown one. Only [`read_session`]
    /// produces that variant.
    fn from(source: OperatorStoreError) -> Self {
        match source {
            OperatorStoreError::ExpiredSessionCleanupFailed => Self::SessionAuthenticationFailed,
            OperatorStoreError::AcquireFailed
            | OperatorStoreError::TransactionFailed
            | OperatorStoreError::AccountReadFailed
            | OperatorStoreError::SessionReadFailed
            | OperatorStoreError::InvalidPersistedFacts
            | OperatorStoreError::SessionInsertFailed
            | OperatorStoreError::SessionDeleteFailed
            | OperatorStoreError::SessionDeleteConflict
            | OperatorStoreError::AuditInsertFailed => Self::PersistenceFailed,
        }
    }
}

impl OperatorStoreError {
    /// A compile-time discriminant for diagnostic logs only. The public
    /// classification is deliberately coarser than this, so it must never reach
    /// a response.
    pub(super) const fn cause(self) -> &'static str {
        match self {
            Self::AcquireFailed => "operator_store_acquire_failed",
            Self::TransactionFailed => "operator_store_transaction_failed",
            Self::AccountReadFailed => "operator_store_account_read_failed",
            Self::SessionReadFailed => "operator_store_session_read_failed",
            Self::ExpiredSessionCleanupFailed => "operator_store_expired_session_cleanup_failed",
            Self::InvalidPersistedFacts => "operator_store_invalid_persisted_facts",
            Self::SessionInsertFailed => "operator_store_session_insert_failed",
            Self::SessionDeleteFailed => "operator_store_session_delete_failed",
            Self::SessionDeleteConflict => "operator_store_session_delete_conflict",
            Self::AuditInsertFailed => "operator_store_audit_insert_failed",
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
