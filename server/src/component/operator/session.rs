use tokio::sync::{Semaphore, SemaphorePermit};
use zeroize::Zeroizing;

use crate::db::{Database, PersistenceError, TransactionError};

mod credentials;

pub(crate) use self::credentials::SessionCredentialHex;
pub(super) use self::credentials::{SessionCredential, SessionCredentialHash};
use super::{
    OperatorError, OperatorIdentity,
    password::{DUMMY_PASSWORD_PHC, OperatorPassword, verify_password_once},
};

// Four complete sign-ins bound both database submissions and Argon2 memory.
// Anonymous callers never queue for capacity.
pub(super) const SIGN_IN_CONCURRENCY: usize = 4;
pub(super) static SIGN_IN_GATE: Semaphore = Semaphore::const_new(SIGN_IN_CONCURRENCY);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SessionFacts {
    pub(super) identity: OperatorIdentity,
    pub(super) expired: bool,
}

enum ExpiredSessionCleanup {
    Missing,
    Live(OperatorIdentity),
    Deleted(OperatorIdentity),
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
    pub(crate) fn wire_credential(&self) -> SessionCredentialHex {
        self.credential.to_wire()
    }
}

/// Establishes an operator session after one frozen-profile password
/// verification, provided its account credential revision is still current at
/// session insertion. Password verification never holds a database transaction.
///
/// # Errors
///
/// Returns a redacted [`OperatorError`] for authentication, persistence,
/// entropy, or blocking-task failures.
pub(super) async fn sign_in(
    database: &Database,
    login_name: &str,
    submitted_password: String,
) -> Result<SignedInSession, OperatorError> {
    let password = OperatorPassword::new(submitted_password);
    super::credentials::validate_input(login_name, password.expose())?;
    let permit = SIGN_IN_GATE
        .try_acquire()
        .map_err(|_| OperatorError::SignInBusy)?;
    let login_name = login_name.to_owned();
    let (account, permit) = database
        .read(move |transaction| {
            crate::component::operator::db::find_account(transaction, &login_name)
                .map(|account| (account, permit))
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(OperatorError::from)?;
    let candidate_phc = account.as_ref().map_or_else(
        || DUMMY_PASSWORD_PHC.to_owned(),
        |facts| facts.password_hash.clone(),
    );

    // Ownership moves through each blocking task and its result. Cancellation
    // cannot free a slot while database or hashing work is queued or running.
    let (password_verified, permit) = tokio::task::spawn_blocking(move || {
        verify_password_once(&password, &candidate_phc).map(|verified| (verified, permit))
    })
    .await
    .map_err(|_| OperatorError::PasswordTaskFailed)??;

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
    record_actor(identity);
    let credential = SessionCredential::generate()?;
    let credential_hash = credential.sha256();
    create_session(
        database,
        &credential_hash,
        identity,
        account.credential_revision,
        permit,
    )
    .await?;

    Ok(SignedInSession {
        identity,
        credential,
    })
}

async fn create_session(
    database: &Database,
    credential_hash: &SessionCredentialHash,
    identity: OperatorIdentity,
    expected_revision: i64,
    permit: SemaphorePermit<'static>,
) -> Result<(), OperatorError> {
    let credential_hash = Zeroizing::new(*credential_hash.as_bytes());
    database
        .write(move |transaction| {
            let inserted = crate::component::operator::db::insert_session_if_current(
                transaction,
                &credential_hash,
                identity,
                expected_revision,
            )?;
            if inserted == 0 {
                return Err(OperatorError::AuthenticationFailed);
            }
            Ok(permit)
        })
        .await
        .map(|_| ())
        .map_err(TransactionError::into_error)
}

/// Authenticates a caller-supplied session credential.
///
/// # Errors
///
/// Missing, malformed, unknown, and expired credentials all return the same
/// typed failure. Persistence failures remain a separate internal cause.
pub(super) async fn authenticate_session(
    database: &Database,
    wire_credential: SessionCredentialHex,
) -> Result<OperatorIdentity, OperatorError> {
    let credential = SessionCredential::from_wire(&wire_credential)
        .map_err(|_| OperatorError::SessionAuthenticationFailed)?;
    let credential_hash = credential.sha256();
    let snapshot_hash = Zeroizing::new(*credential_hash.as_bytes());
    let Some(facts) = database
        .read(move |transaction| {
            crate::component::operator::db::find_session(transaction, &snapshot_hash)
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(OperatorError::from)?
    else {
        return Err(OperatorError::SessionAuthenticationFailed);
    };
    record_actor(facts.identity);
    if !facts.expired {
        return Ok(facts.identity);
    }

    let cleanup_hash = Zeroizing::new(*credential_hash.as_bytes());
    let cleanup = database
        .write(move |transaction| {
            let Some(current) =
                crate::component::operator::db::find_session(transaction, &cleanup_hash)?
            else {
                return Ok(ExpiredSessionCleanup::Missing);
            };
            if !current.expired {
                return Ok(ExpiredSessionCleanup::Live(current.identity));
            }
            let deleted =
                crate::component::operator::db::delete_session_by_hash(transaction, &cleanup_hash)?;
            if deleted != 1 {
                return Err(PersistenceError::InvalidPersistedData);
            }
            Ok(ExpiredSessionCleanup::Deleted(current.identity))
        })
        .await
        .map_err(TransactionError::into_error);

    match cleanup {
        Ok(ExpiredSessionCleanup::Live(identity)) => Ok(identity),
        Ok(ExpiredSessionCleanup::Deleted(identity)) => {
            record_actor(identity);
            Err(OperatorError::SessionAuthenticationFailed)
        }
        Ok(ExpiredSessionCleanup::Missing) => Err(OperatorError::SessionAuthenticationFailed),
        Err(_) => {
            tracing::warn!(
                cause = "operator_store_transaction_failed",
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
pub(super) async fn terminate_session(
    database: &Database,
    wire_credential: SessionCredentialHex,
) -> Result<(), OperatorError> {
    let Ok(credential) = SessionCredential::from_wire(&wire_credential) else {
        return Ok(());
    };
    let credential_hash = credential.sha256();
    let credential_hash = Zeroizing::new(*credential_hash.as_bytes());
    let identity = database
        .write(move |transaction| {
            let Some(current) =
                crate::component::operator::db::find_session(transaction, &credential_hash)?
            else {
                return Ok(None);
            };
            let deleted = crate::component::operator::db::delete_session_by_hash(
                transaction,
                &credential_hash,
            )?;
            if deleted != 1 {
                return Err(PersistenceError::InvalidPersistedData);
            }
            Ok(Some(current.identity))
        })
        .await
        .map_err(TransactionError::into_error)
        .map_err(OperatorError::from)?;
    if let Some(identity) = identity {
        record_actor(identity);
    }
    Ok(())
}

fn record_actor(identity: OperatorIdentity) {
    let actor_id = identity.operator_id().to_string();
    let span = tracing::Span::current();
    span.record("actor_kind", "operator");
    span.record("actor_id", actor_id.as_str());
}

#[cfg(test)]
pub(in crate::component::operator) mod tests {
    pub(in crate::component::operator) use super::credentials::{
        SESSION_CREDENTIAL_LENGTH, decode_lower_hex,
    };
    use super::{SessionCredential, SignedInSession};

    impl SignedInSession {
        pub(in crate::component::operator) const fn credential_for_test(
            &self,
        ) -> &SessionCredential {
            &self.credential
        }
    }
}
