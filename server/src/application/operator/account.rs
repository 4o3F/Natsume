use uuid::Uuid;

use crate::{
    audit::{AuditEvent, AuditEventId, CorrelationId},
    db::{self, Database},
};

use super::{OperatorError, OperatorIdentity, OperatorRole};

pub(crate) struct AccountFacts {
    pub(crate) identity: OperatorIdentity,
    pub(crate) password_hash: String,
}

/// Creates the only bootstrap administrator and its audit in one write
/// transaction.
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
}

pub(crate) async fn create_first_admin_with_ids(
    database: &Database,
    login_name: &str,
    password_hash: &str,
    operator_id: Uuid,
    audit_event_id: AuditEventId,
    correlation_id: CorrelationId,
) -> Result<Uuid, OperatorError> {
    let login_name = login_name.to_owned();
    let password_hash = password_hash.to_owned();
    database
        .write(move |transaction| {
            if db::operator::any_account_exists(transaction)? {
                return Err(OperatorError::PersistenceFailed);
            }
            db::operator::insert_account(
                transaction,
                operator_id,
                &login_name,
                OperatorRole::Admin,
                &password_hash,
            )?;
            let event =
                AuditEvent::first_admin_created(audit_event_id, correlation_id, operator_id);
            db::audit::insert(transaction, &event)
                .map_err(OperatorError::from_audit_persistence)?;
            Ok(operator_id)
        })
        .await
}

/// Replaces one operator password, removes exactly that operator's sessions,
/// and records the removed count atomically.
pub(crate) async fn reset_operator_password(
    database: &Database,
    login_name: &str,
    password_hash: &str,
) -> Result<(), OperatorError> {
    let correlation_id = CorrelationId::from_uuid(Uuid::now_v7());
    let result = reset_operator_password_with_ids(
        database,
        login_name,
        password_hash,
        AuditEventId::from_uuid(Uuid::now_v7()),
        correlation_id,
    )
    .await;
    if result.is_err() {
        tracing::warn!(
            correlation_id = %correlation_id.as_text(),
            "operator password reset failed"
        );
    }
    result
}

pub(crate) async fn reset_operator_password_with_ids(
    database: &Database,
    login_name: &str,
    password_hash: &str,
    audit_event_id: AuditEventId,
    correlation_id: CorrelationId,
) -> Result<(), OperatorError> {
    let login_name = login_name.to_owned();
    let password_hash = password_hash.to_owned();
    database
        .write(move |transaction| {
            let account = db::operator::find_account(transaction, &login_name)?
                .ok_or(OperatorError::PersistenceFailed)?;
            db::operator::update_password(
                transaction,
                account.identity.operator_id(),
                &password_hash,
            )?;
            let removed_session_count = db::operator::delete_sessions_by_operator(
                transaction,
                account.identity.operator_id(),
            )?;
            let event = AuditEvent::operator_password_reset(
                audit_event_id,
                correlation_id,
                account.identity.operator_id(),
                removed_session_count,
            );
            db::audit::insert(transaction, &event).map_err(OperatorError::from_audit_persistence)
        })
        .await
}
