use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    audit::{AuditEvent, AuditEventId, CorrelationId},
    db::Database,
};

mod credentials;

pub(crate) use self::credentials::{
    OperatorCredentials, SessionCredential, SessionCredentialHash, SessionCredentialHex,
};
#[cfg(test)]
pub(super) use self::credentials::{SESSION_CREDENTIAL_LENGTH, decode_lower_hex};
use super::{
    OperatorError, OperatorIdentity,
    password::{
        DUMMY_PASSWORD_PHC, OperatorPassword, PASSWORD_VERIFICATION_GATE, verify_password_once,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionFacts {
    pub(crate) identity: OperatorIdentity,
    pub(crate) expired: bool,
}

enum ExpiredSessionCleanup {
    Missing,
    Live(OperatorIdentity),
    Deleted,
}

pub(crate) struct SignedInSession {
    identity: OperatorIdentity,
    credential: SessionCredential,
}

impl SignedInSession {
    #[must_use]
    pub(crate) const fn identity(&self) -> OperatorIdentity {
        self.identity
    }

    #[must_use]
    pub(crate) const fn credential(&self) -> &SessionCredential {
        &self.credential
    }
}

/// Establishes an operator session after one frozen-profile password
/// verification.
///
/// # Errors
///
/// Returns a redacted [`OperatorError`] for authentication, persistence,
/// entropy, or blocking-task failures.
pub(crate) async fn sign_in(
    database: &Database,
    correlation_id: CorrelationId,
    login_name: &str,
    submitted_password: String,
) -> Result<SignedInSession, OperatorError> {
    let password = OperatorPassword::new(submitted_password);
    let login_name = login_name.to_owned();
    let account = database
        .read(move |transaction| {
            crate::component::operator::db::find_account(transaction, &login_name)
        })
        .await?;
    let candidate_phc = account.as_ref().map_or_else(
        || DUMMY_PASSWORD_PHC.to_owned(),
        |facts| facts.password_hash.clone(),
    );

    // The permit is taken after the account read so both the known-login and
    // unknown-login paths pass through the same gate, preserving the timing
    // equalization below. The wait queue is unbounded on purpose: Argon2 memory
    // is only allocated inside the blocking closure, so queued waiters are
    // cheap, and connection capacity stays a separate deferred question. A
    // `static` semaphore is never closed, so the only reachable acquire failure
    // is treated as a blocking-task failure.
    let verification = {
        let _permit = PASSWORD_VERIFICATION_GATE
            .acquire()
            .await
            .map_err(|_| OperatorError::PasswordTaskFailed)?;
        tokio::task::spawn_blocking(move || verify_password_once(&password, &candidate_phc))
            .await
            .map_err(|_| OperatorError::PasswordTaskFailed)?
    };
    let password_verified = verification?;

    // The unknown-login path verifies the fixed dummy PHC to equalize the
    // expensive work, but it can never authenticate: the result is discarded
    // unless an account row was actually returned.
    let Some(account) = account else {
        return Err(OperatorError::AuthenticationFailed);
    };
    if !password_verified {
        return Err(OperatorError::AuthenticationFailed);
    }

    let identity = account.identity;
    let credential = SessionCredential::generate()?;
    let credential_hash = credential.sha256();
    create_session(database, &credential_hash, identity, correlation_id).await?;

    Ok(SignedInSession {
        identity,
        credential,
    })
}

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
}

pub(crate) async fn create_session_with_audit_id(
    database: &Database,
    credential_hash: &SessionCredentialHash,
    identity: OperatorIdentity,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), OperatorError> {
    let credential_hash = Zeroizing::new(*credential_hash.as_bytes());
    database
        .write(move |transaction| {
            crate::component::operator::db::insert_session(
                transaction,
                &credential_hash,
                identity,
            )?;
            let event = AuditEvent::session_established(
                audit_event_id,
                correlation_id,
                identity.operator_id(),
                identity.role().as_persisted(),
            );
            crate::audit::insert(transaction, &event).map_err(OperatorError::from_audit_persistence)
        })
        .await
}

/// Authenticates a caller-supplied session credential.
///
/// # Errors
///
/// Missing, malformed, unknown, and expired credentials all return the same
/// typed failure. Persistence failures remain a separate internal cause.
pub(crate) async fn authenticate_session(
    database: &Database,
    correlation_id: CorrelationId,
    wire_credential: SessionCredentialHex,
) -> Result<OperatorIdentity, OperatorError> {
    let credential = SessionCredential::from_wire(&wire_credential)
        .map_err(|_| OperatorError::SessionAuthenticationFailed)?;
    let credential_hash = credential.sha256();
    let snapshot_hash = Zeroizing::new(*credential_hash.as_bytes());
    let Some(facts) = database
        .read(move |transaction| {
            crate::component::operator::db::query::find_session(transaction, &snapshot_hash)
        })
        .await?
    else {
        return Err(OperatorError::SessionAuthenticationFailed);
    };
    if !facts.expired {
        return Ok(facts.identity);
    }

    let cleanup_hash = Zeroizing::new(*credential_hash.as_bytes());
    let cleanup = database
        .write(move |transaction| {
            let Some(current) =
                crate::component::operator::db::query::find_session(transaction, &cleanup_hash)?
            else {
                return Ok(ExpiredSessionCleanup::Missing);
            };
            if !current.expired {
                return Ok(ExpiredSessionCleanup::Live(current.identity));
            }
            let deleted =
                crate::component::operator::db::delete_session_by_hash(transaction, &cleanup_hash)?;
            if deleted != 1 {
                return Err(OperatorError::PersistenceFailed);
            }
            let event = AuditEvent::session_expired(
                AuditEventId::from_uuid(Uuid::now_v7()),
                correlation_id,
                current.identity.operator_id(),
            );
            crate::audit::insert(transaction, &event)
                .map_err(OperatorError::from_audit_persistence)?;
            Ok(ExpiredSessionCleanup::Deleted)
        })
        .await;

    match cleanup {
        Ok(ExpiredSessionCleanup::Live(identity)) => Ok(identity),
        Ok(ExpiredSessionCleanup::Missing | ExpiredSessionCleanup::Deleted) => {
            Err(OperatorError::SessionAuthenticationFailed)
        }
        Err(_) => {
            tracing::warn!(
                cause = "operator_store_transaction_failed",
                correlation_id = %correlation_id.as_text(),
                "expired operator session cleanup failed"
            );
            Err(OperatorError::SessionAuthenticationFailed)
        }
    }
}

/// Terminates a session if it exists and is live.
///
/// Malformed, missing, unknown, and already-deleted credentials are successful
/// zero-write no-ops.
///
/// # Errors
///
/// Returns a redacted [`OperatorError`] only for internal persistence failure.
pub(crate) async fn terminate_session(
    database: &Database,
    correlation_id: CorrelationId,
    wire_credential: SessionCredentialHex,
) -> Result<(), OperatorError> {
    let Ok(credential) = SessionCredential::from_wire(&wire_credential) else {
        return Ok(());
    };
    let credential_hash = credential.sha256();
    terminate_session_with_audit_id(
        database,
        &credential_hash,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
}

pub(crate) async fn terminate_session_with_audit_id(
    database: &Database,
    credential_hash: &SessionCredentialHash,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), OperatorError> {
    let credential_hash = Zeroizing::new(*credential_hash.as_bytes());
    database
        .write(move |transaction| {
            let Some(current) =
                crate::component::operator::db::query::find_session(transaction, &credential_hash)?
            else {
                return Ok(());
            };
            let deleted = crate::component::operator::db::delete_session_by_hash(
                transaction,
                &credential_hash,
            )?;
            if deleted != 1 {
                return Err(OperatorError::PersistenceFailed);
            }
            let event = if current.expired {
                AuditEvent::session_expired(
                    audit_event_id,
                    correlation_id,
                    current.identity.operator_id(),
                )
            } else {
                AuditEvent::session_terminated(
                    audit_event_id,
                    correlation_id,
                    current.identity.operator_id(),
                )
            };
            crate::audit::insert(transaction, &event).map_err(OperatorError::from_audit_persistence)
        })
        .await
}
