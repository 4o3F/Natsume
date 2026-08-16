use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, sqlite::SqliteConnection,
};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    application::operator::OperatorError,
    audit::{self, AuditEvent, AuditEventId, CorrelationId},
    db::{
        Database,
        schema::{operator_accounts, operator_sessions},
    },
};

/// Updates one existing operator password, removes every current session for
/// that operator, and persists the recovery audit evidence atomically.
///
/// # Errors
///
/// Returns a redacted [`OperatorError`] when the login name is unknown or any
/// transaction stage fails.
pub(crate) async fn reset_operator_password(
    database: &Database,
    login_name: &str,
    password_hash: &str,
) -> Result<(), OperatorError> {
    let audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    let correlation_id = CorrelationId::from_uuid(Uuid::now_v7());
    reset_operator_password_with_ids(
        database,
        login_name,
        password_hash,
        audit_event_id,
        correlation_id,
    )
    .await
    .map_err(|error| {
        tracing::warn!(
            cause = error.cause(),
            correlation_id = %correlation_id.as_text(),
            "operator password reset failed"
        );
        OperatorError::from(error)
    })
}

pub(super) async fn reset_operator_password_with_ids(
    database: &Database,
    login_name: &str,
    password_hash: &str,
    audit_event_id: AuditEventId,
    correlation_id: CorrelationId,
) -> Result<(), ResetOperatorPasswordError> {
    let login_name = login_name.to_owned();
    let password_hash = password_hash.to_owned();
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                reset_operator_password_in_transaction(
                    connection,
                    &login_name,
                    &password_hash,
                    audit_event_id,
                    correlation_id,
                )
            })
        })
        .await
        .map_err(|_| ResetOperatorPasswordError::DatabaseAcquireFailed)?
}

pub(super) fn reset_operator_password_in_transaction(
    connection: &mut SqliteConnection,
    login_name: &str,
    password_hash: &str,
    audit_event_id: AuditEventId,
    correlation_id: CorrelationId,
) -> Result<(), ResetOperatorPasswordError> {
    let operator_id = operator_accounts::table
        .filter(operator_accounts::login_name.eq(login_name))
        .select(operator_accounts::operator_id)
        .first::<String>(connection)
        .optional()
        .map_err(|_| ResetOperatorPasswordError::TargetReadFailed)?
        .ok_or(ResetOperatorPasswordError::TargetNotFound)?;

    let updated = diesel::update(
        operator_accounts::table.filter(operator_accounts::operator_id.eq(&operator_id)),
    )
    .set(operator_accounts::password_hash.eq(password_hash))
    .execute(connection)
    .map_err(|_| ResetOperatorPasswordError::PasswordUpdateFailed)?;
    if updated != 1 {
        return Err(ResetOperatorPasswordError::PasswordUpdateConflict);
    }

    let removed_session_count = diesel::delete(
        operator_sessions::table.filter(operator_sessions::operator_id.eq(&operator_id)),
    )
    .execute(connection)
    .map_err(|_| ResetOperatorPasswordError::SessionsPurgeFailed)?;
    let event = AuditEvent::operator_password_reset(
        audit_event_id,
        correlation_id,
        operator_id,
        removed_session_count,
    );
    audit::insert_diesel(connection, &event)
        .map_err(|_| ResetOperatorPasswordError::AuditPersistenceFailed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(super) enum ResetOperatorPasswordError {
    #[snafu(display("the password-reset database connection could not be acquired"))]
    DatabaseAcquireFailed,
    #[snafu(display("the password-reset transaction failed"))]
    TransactionControlFailed,
    #[snafu(display("the password-reset operator account could not be read"))]
    TargetReadFailed,
    #[snafu(display("the password-reset operator account does not exist"))]
    TargetNotFound,
    #[snafu(display("the operator password could not be updated"))]
    PasswordUpdateFailed,
    #[snafu(display("the operator password update changed an unexpected number of rows"))]
    PasswordUpdateConflict,
    #[snafu(display("the operator sessions could not be removed"))]
    SessionsPurgeFailed,
    #[snafu(display("the password-reset audit could not be persisted"))]
    AuditPersistenceFailed,
}

impl From<diesel::result::Error> for ResetOperatorPasswordError {
    fn from(_source: diesel::result::Error) -> Self {
        Self::TransactionControlFailed
    }
}

impl ResetOperatorPasswordError {
    pub(super) const fn cause(self) -> &'static str {
        match self {
            Self::DatabaseAcquireFailed => "password_reset_database_acquire_failed",
            Self::TransactionControlFailed => "password_reset_transaction_failed",
            Self::TargetReadFailed => "password_reset_account_read_failed",
            Self::TargetNotFound => "password_reset_account_not_found",
            Self::PasswordUpdateFailed => "password_reset_account_update_failed",
            Self::PasswordUpdateConflict => "password_reset_account_update_conflict",
            Self::SessionsPurgeFailed => "password_reset_session_delete_failed",
            Self::AuditPersistenceFailed => "password_reset_audit_insert_failed",
        }
    }
}

impl From<ResetOperatorPasswordError> for OperatorError {
    fn from(_source: ResetOperatorPasswordError) -> Self {
        Self::PersistenceFailed
    }
}
