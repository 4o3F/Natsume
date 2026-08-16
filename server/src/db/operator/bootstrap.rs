use diesel::{
    ExpressionMethods, QueryDsl, RunQueryDsl,
    dsl::{exists, select},
    sqlite::SqliteConnection,
};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    application::operator::{OperatorError, OperatorRole},
    audit::{self, AuditEvent, AuditEventId, CorrelationId},
    db::{Database, schema::operator_accounts},
};

/// Creates the single first administrator and its audit evidence atomically.
///
/// # Errors
///
/// Returns a redacted [`OperatorError`] when an account already exists or any
/// transaction stage fails.
pub(crate) async fn create_first_admin(
    database: &Database,
    login_name: &str,
    password_hash: &str,
) -> Result<Uuid, OperatorError> {
    create_first_admin_with_ids(
        database,
        login_name,
        password_hash,
        Uuid::now_v7(),
        AuditEventId::from_uuid(Uuid::now_v7()),
        CorrelationId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(OperatorError::from)
}

pub(super) async fn create_first_admin_with_ids(
    database: &Database,
    login_name: &str,
    password_hash: &str,
    operator_id: Uuid,
    audit_event_id: AuditEventId,
    correlation_id: CorrelationId,
) -> Result<Uuid, CreateFirstAdminError> {
    let login_name = login_name.to_owned();
    let password_hash = password_hash.to_owned();
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                create_first_admin_in_transaction(
                    connection,
                    &login_name,
                    &password_hash,
                    operator_id,
                    audit_event_id,
                    correlation_id,
                )
            })
        })
        .await
        .map_err(|_| CreateFirstAdminError::AcquireFailed)?
}

pub(super) fn create_first_admin_in_transaction(
    connection: &mut SqliteConnection,
    login_name: &str,
    password_hash: &str,
    operator_id: Uuid,
    audit_event_id: AuditEventId,
    correlation_id: CorrelationId,
) -> Result<Uuid, CreateFirstAdminError> {
    let account_exists = select(exists(
        operator_accounts::table.select(operator_accounts::operator_id),
    ))
    .get_result::<bool>(connection)
    .map_err(|_| CreateFirstAdminError::ReadFailed)?;
    if account_exists {
        return Err(CreateFirstAdminError::AccountAlreadyExists);
    }

    diesel::insert_into(operator_accounts::table)
        .values((
            operator_accounts::operator_id.eq(operator_id.to_string()),
            operator_accounts::login_name.eq(login_name),
            operator_accounts::role.eq(OperatorRole::Admin.as_persisted()),
            operator_accounts::password_hash.eq(password_hash),
        ))
        .execute(connection)
        .map_err(|_| CreateFirstAdminError::AccountInsertFailed)?;

    let event = AuditEvent::first_admin_created(audit_event_id, correlation_id, operator_id);
    audit::insert_diesel(connection, &event)
        .map_err(|_| CreateFirstAdminError::AuditInsertFailed)?;
    Ok(operator_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(super) enum CreateFirstAdminError {
    #[snafu(display("the first-admin database connection could not be acquired"))]
    AcquireFailed,
    #[snafu(display("the first-admin transaction failed"))]
    TransactionFailed,
    #[snafu(display("the operator account set could not be read"))]
    ReadFailed,
    #[snafu(display("an operator account already exists"))]
    AccountAlreadyExists,
    #[snafu(display("the first administrator could not be persisted"))]
    AccountInsertFailed,
    #[snafu(display("the first-administrator audit could not be persisted"))]
    AuditInsertFailed,
}

impl From<diesel::result::Error> for CreateFirstAdminError {
    /// Transaction control is the only stage that reports a raw Diesel error,
    /// and the source is discarded so no SQL text can reach a log or response.
    fn from(_source: diesel::result::Error) -> Self {
        Self::TransactionFailed
    }
}

impl From<CreateFirstAdminError> for OperatorError {
    /// Bootstrap is the only caller and collapses every outcome into one
    /// composition-root failure, so the typed first-admin vocabulary above stays
    /// inside this module instead of becoming application vocabulary.
    fn from(_source: CreateFirstAdminError) -> Self {
        Self::PersistenceFailed
    }
}
