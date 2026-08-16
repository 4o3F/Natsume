use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, QueryableByName, RunQueryDsl,
    dsl::sql,
    sql_types::{BigInt, Binary, Text},
    sqlite::SqliteConnection,
};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    application::operator::{
        AccountFacts, OperatorError, OperatorIdentity, SessionCredentialHash, SessionFacts,
    },
    audit::{self, AuditEvent, AuditEventId, CorrelationId},
    db::{
        Database,
        schema::{operator_accounts, operator_sessions},
    },
};

use super::OperatorStoreError;

/// Reads the minimal persisted facts for one exact login name.
///
/// # Errors
///
/// Returns a redacted [`OperatorError`] when the query fails.
pub(crate) async fn read_account(
    database: &Database,
    login_name: &str,
) -> Result<Option<AccountFacts>, OperatorError> {
    let login_name = login_name.to_owned();
    database
        .interact(move |connection| {
            operator_accounts::table
                .filter(operator_accounts::login_name.eq(login_name))
                .select((
                    operator_accounts::operator_id,
                    operator_accounts::role,
                    operator_accounts::password_hash,
                ))
                .first::<(String, String, String)>(connection)
                .optional()
                .map(|row| {
                    row.map(|(operator_id, role, password_hash)| AccountFacts {
                        operator_id,
                        role,
                        password_hash,
                    })
                })
                .map_err(|_| OperatorStoreError::AccountReadFailed)
        })
        .await
        .map_err(|_| OperatorStoreError::AcquireFailed)?
        .map_err(OperatorError::from)
}

/// Persists a session and its establishment audit atomically.
///
/// # Errors
///
/// Returns a redacted [`OperatorError`] if any transaction stage fails.
pub(crate) async fn create_session(
    database: &Database,
    credential_hash: &SessionCredentialHash,
    identity: OperatorIdentity,
    correlation_id: CorrelationId,
) -> Result<(), OperatorError> {
    create_session_with_audit_id(
        database,
        credential_hash,
        identity,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(OperatorError::from)
}

pub(super) async fn create_session_with_audit_id(
    database: &Database,
    credential_hash: &SessionCredentialHash,
    identity: OperatorIdentity,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), OperatorStoreError> {
    let credential_hash = Zeroizing::new(*credential_hash.as_bytes());
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                create_session_in_transaction(
                    connection,
                    credential_hash.as_slice(),
                    identity,
                    correlation_id,
                    audit_event_id,
                )
            })
        })
        .await
        .map_err(|_| OperatorStoreError::AcquireFailed)?
}

pub(super) fn create_session_in_transaction(
    connection: &mut SqliteConnection,
    credential_hash: &[u8],
    identity: OperatorIdentity,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), OperatorStoreError> {
    diesel::insert_into(operator_sessions::table)
        .values((
            operator_sessions::session_credential_hash.eq(credential_hash),
            operator_sessions::operator_id.eq(identity.operator_id().to_string()),
            operator_sessions::expires_at.eq(sql::<Text>(
                "strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+57600 seconds')",
            )),
        ))
        .execute(connection)
        .map_err(|_| OperatorStoreError::SessionInsertFailed)?;

    let event = AuditEvent::session_established(
        audit_event_id,
        correlation_id,
        identity.operator_id(),
        identity.role().as_persisted(),
    );
    audit::insert_diesel(connection, &event).map_err(|_| OperatorStoreError::AuditInsertFailed)
}

/// Reads a session and lazily removes it with audit evidence if expired.
///
/// # Errors
///
/// Returns a redacted [`OperatorError`] if the read, guarded expiry, or
/// transaction finalization fails.
pub(crate) async fn read_session(
    database: &Database,
    credential_hash: &SessionCredentialHash,
    correlation_id: CorrelationId,
) -> Result<Option<SessionFacts>, OperatorError> {
    let credential_hash = Zeroizing::new(*credential_hash.as_bytes());
    database
        .interact(move |connection| {
            read_session_on_connection(connection, credential_hash.as_slice(), correlation_id)
        })
        .await
        .map_err(|_| OperatorStoreError::AcquireFailed)?
        .map_err(OperatorError::from)
}

/// Authentication reads every protected request, so the common live-session
/// case stays outside any transaction and never takes the `SQLite` write lock.
/// Only the rare expiry transition escalates to `BEGIN IMMEDIATE`.
pub(super) fn read_session_on_connection(
    connection: &mut SqliteConnection,
    credential_hash: &[u8],
    correlation_id: CorrelationId,
) -> Result<Option<SessionFacts>, OperatorStoreError> {
    let Some(row) = read_session_row(connection, credential_hash)? else {
        return Ok(None);
    };
    if !row.expired {
        return Ok(Some(session_facts(row)));
    }
    // The row was already observed expired, so the caller can classify this
    // request without the cleanup succeeding. That classification swallows the
    // internal cause, so record it here.
    let cleanup = connection.immediate_transaction(|connection| {
        expire_session_in_transaction(connection, credential_hash, correlation_id)
    });
    match cleanup {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            tracing::warn!(
                cause = error.cause(),
                correlation_id = %correlation_id.as_text(),
                "expired operator session cleanup failed"
            );
            Err(OperatorStoreError::ExpiredSessionCleanupFailed)
        }
    }
}

/// Re-reads under the write lock so exactly one concurrent expirer emits the
/// expiry audit; the losers observe the deleted row and return `Missing`.
pub(super) fn expire_session_in_transaction(
    connection: &mut SqliteConnection,
    credential_hash: &[u8],
    correlation_id: CorrelationId,
) -> Result<Option<SessionFacts>, OperatorStoreError> {
    let Some(row) = read_session_row(connection, credential_hash)? else {
        return Ok(None);
    };
    if !row.expired {
        return Ok(Some(session_facts(row)));
    }
    let operator_id =
        Uuid::parse_str(&row.operator_id).map_err(|_| OperatorStoreError::InvalidPersistedFacts)?;
    let event = AuditEvent::session_expired(
        AuditEventId::from_uuid(Uuid::now_v7()),
        correlation_id,
        operator_id,
    );
    delete_session_with_audit(connection, credential_hash, &event)?;
    Ok(None)
}

pub(super) fn session_facts(row: SessionRow) -> SessionFacts {
    SessionFacts {
        operator_id: row.operator_id,
        role: row.role,
    }
}

/// Deletes a live session once and audits the transition once. Missing rows are
/// zero-write no-ops; expired rows use the expiry audit vocabulary.
///
/// # Errors
///
/// Returns a redacted [`OperatorError`] if an internal transaction stage fails.
pub(crate) async fn terminate_session(
    database: &Database,
    credential_hash: &SessionCredentialHash,
    correlation_id: CorrelationId,
) -> Result<(), OperatorError> {
    terminate_session_with_audit_id(
        database,
        credential_hash,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(OperatorError::from)
}

pub(super) async fn terminate_session_with_audit_id(
    database: &Database,
    credential_hash: &SessionCredentialHash,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), OperatorStoreError> {
    let credential_hash = Zeroizing::new(*credential_hash.as_bytes());
    database
        .interact(move |connection| {
            // `BEGIN IMMEDIATE` opens before the read, and the no-row case still
            // commits, so a database failure never correlates with whether the
            // session exists.
            connection.immediate_transaction(|connection| {
                terminate_session_in_transaction(
                    connection,
                    credential_hash.as_slice(),
                    correlation_id,
                    audit_event_id,
                )
            })
        })
        .await
        .map_err(|_| OperatorStoreError::AcquireFailed)?
}

pub(super) fn terminate_session_in_transaction(
    connection: &mut SqliteConnection,
    credential_hash: &[u8],
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), OperatorStoreError> {
    let Some(row) = read_session_row(connection, credential_hash)? else {
        return Ok(());
    };
    let operator_id =
        Uuid::parse_str(&row.operator_id).map_err(|_| OperatorStoreError::InvalidPersistedFacts)?;
    if row.expired {
        let event = AuditEvent::session_expired(audit_event_id, correlation_id, operator_id);
        delete_session_with_audit(connection, credential_hash, &event)
    } else {
        let event = AuditEvent::session_terminated(audit_event_id, correlation_id, operator_id);
        delete_session_with_audit(connection, credential_hash, &event)
    }
}

pub(super) struct SessionRow {
    pub(super) operator_id: String,
    pub(super) role: String,
    pub(super) expired: bool,
}

#[derive(QueryableByName)]
pub(super) struct PersistedSessionRow {
    #[diesel(sql_type = Text)]
    pub(super) operator_id: String,
    #[diesel(sql_type = Text)]
    pub(super) role: String,
    #[diesel(sql_type = BigInt)]
    pub(super) expiry_valid: i64,
    #[diesel(sql_type = BigInt)]
    pub(super) expired: i64,
}

pub(super) fn read_session_row(
    connection: &mut SqliteConnection,
    credential_hash: &[u8],
) -> Result<Option<SessionRow>, OperatorStoreError> {
    let row = diesel::sql_query(
        "SELECT accounts.operator_id AS operator_id, accounts.role AS role, \
         CASE WHEN julianday(sessions.expires_at) IS NULL THEN 0 ELSE 1 END AS expiry_valid, \
         CASE WHEN julianday(sessions.expires_at) <= julianday('now') THEN 1 ELSE 0 END AS expired \
         FROM operator_sessions AS sessions \
         INNER JOIN operator_accounts AS accounts ON accounts.operator_id = sessions.operator_id \
         WHERE sessions.session_credential_hash = ?",
    )
    .bind::<Binary, _>(credential_hash)
    .get_result::<PersistedSessionRow>(connection)
    .optional()
    .map_err(|_| OperatorStoreError::SessionReadFailed)?;
    row.map(|row| {
        if !matches!(row.expiry_valid, 0 | 1) || !matches!(row.expired, 0 | 1) {
            return Err(OperatorStoreError::InvalidPersistedFacts);
        }
        if row.expiry_valid == 0 {
            return Err(OperatorStoreError::InvalidPersistedFacts);
        }
        Ok(SessionRow {
            operator_id: row.operator_id,
            role: row.role,
            expired: row.expired == 1,
        })
    })
    .transpose()
}

pub(super) fn delete_session_with_audit(
    connection: &mut SqliteConnection,
    credential_hash: &[u8],
    event: &AuditEvent,
) -> Result<(), OperatorStoreError> {
    let result = diesel::delete(
        operator_sessions::table
            .filter(operator_sessions::session_credential_hash.eq(credential_hash)),
    )
    .execute(connection)
    .map_err(|_| OperatorStoreError::SessionDeleteFailed)?;
    if result != 1 {
        return Err(OperatorStoreError::SessionDeleteConflict);
    }
    audit::insert_diesel(connection, event).map_err(|_| OperatorStoreError::AuditInsertFailed)
}
