use snafu::Snafu;
use uuid::Uuid;

use crate::db::Database;

mod account;
mod db;
mod password;
mod session;

#[cfg(test)]
pub(crate) mod test_db {
    use uuid::Uuid;

    use crate::db::Database;

    use super::{OperatorError, OperatorRole};

    pub(crate) async fn test_expire_all_sessions(database: &Database) -> Result<(), OperatorError> {
        super::db::tests::test_expire_all_sessions(database).await
    }

    pub(crate) async fn test_insert_admin_account(
        database: &Database,
        login_name: &str,
        password_hash: &str,
    ) -> Result<Uuid, OperatorError> {
        super::db::tests::test_insert_account(
            database,
            login_name,
            OperatorRole::Admin,
            password_hash,
        )
        .await
    }
}

use self::account::AccountFacts;
#[cfg(test)]
use self::password::OperatorPassword;
#[cfg(test)]
use self::password::{
    DUMMY_PASSWORD_PHC, PASSWORD_VERIFICATION_CONCURRENCY, PASSWORD_VERIFICATION_GATE,
    hash_password as hash_raw_password, verify_password_once,
};
#[cfg(test)]
use self::session::SessionCredential;
use self::session::SessionFacts;
pub(crate) use self::session::{OperatorCredentials, SessionCredentialHex, SignedInSession};
#[cfg(test)]
use self::session::{SESSION_CREDENTIAL_LENGTH, authenticate_session, decode_lower_hex, sign_in};
#[cfg(test)]
pub(crate) use self::tests::PasswordVerificationTestGuard;

/// Operator authentication and account authority with private persistence.
pub(crate) struct OperatorComponent {
    database: Database,
}

impl OperatorComponent {
    pub(crate) const fn new(database: Database) -> Self {
        Self { database }
    }

    pub(crate) async fn create_first_admin(
        &self,
        login_name: &str,
        password_hash: &str,
    ) -> Result<Uuid, OperatorError> {
        account::create_first_admin(&self.database, login_name, password_hash).await
    }

    pub(crate) async fn reset_password(
        &self,
        login_name: &str,
        password_hash: &str,
    ) -> Result<(), OperatorError> {
        account::reset_operator_password(&self.database, login_name, password_hash).await
    }

    pub(crate) async fn sign_in(
        &self,
        login_name: &str,
        submitted_password: String,
    ) -> Result<SignedInSession, OperatorError> {
        session::sign_in(&self.database, login_name, submitted_password).await
    }

    pub(crate) async fn authenticate_session(
        &self,
        wire_credential: SessionCredentialHex,
    ) -> Result<OperatorIdentity, OperatorError> {
        session::authenticate_session(&self.database, wire_credential).await
    }

    pub(crate) async fn terminate_session(
        &self,
        wire_credential: SessionCredentialHex,
    ) -> Result<(), OperatorError> {
        session::terminate_session(&self.database, wire_credential).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperatorRole {
    Admin,
    Viewer,
}

impl OperatorRole {
    #[must_use]
    const fn as_persisted(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Viewer => "viewer",
        }
    }

    /// Converts a persisted role string into the closed role type.
    ///
    /// # Errors
    ///
    /// Returns [`OperatorError::InvalidPersistedRole`] for unknown values.
    fn from_persisted(value: &str) -> Result<Self, OperatorError> {
        match value {
            "admin" => Ok(Self::Admin),
            "viewer" => Ok(Self::Viewer),
            _ => Err(OperatorError::InvalidPersistedRole),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OperatorIdentity {
    operator_id: Uuid,
    role: OperatorRole,
}

impl OperatorIdentity {
    fn from_persisted(operator_id: &str, role: &str) -> Result<Self, OperatorError> {
        Ok(Self {
            operator_id: Uuid::parse_str(operator_id)
                .map_err(|_| OperatorError::InvalidPersistedIdentity)?,
            role: OperatorRole::from_persisted(role)?,
        })
    }

    #[must_use]
    pub(crate) const fn operator_id(self) -> Uuid {
        self.operator_id
    }

    #[must_use]
    #[cfg(test)]
    const fn role(self) -> OperatorRole {
        self.role
    }

    #[must_use]
    pub(crate) const fn role_name(self) -> &'static str {
        self.role.as_persisted()
    }

    pub(crate) const fn require_admin(self) -> Result<(), OperatorError> {
        require_admin(self.role)
    }
}

/// Applies the closed two-role authorization policy.
///
/// # Errors
///
/// Returns [`OperatorError::AuthorizationDenied`] for a viewer.
const fn require_admin(role: OperatorRole) -> Result<(), OperatorError> {
    match role {
        OperatorRole::Admin => Ok(()),
        OperatorRole::Viewer => Err(OperatorError::AuthorizationDenied),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum OperatorError {
    #[snafu(display("operator authentication failed"))]
    AuthenticationFailed,
    #[snafu(display("operator session authentication failed"))]
    SessionAuthenticationFailed,
    #[snafu(display("operator authorization was denied"))]
    AuthorizationDenied,
    #[snafu(display("operator persistence failed"))]
    PersistenceFailed,
    #[snafu(display("the operator password task failed"))]
    PasswordTaskFailed,
    #[snafu(display("operator password verification failed"))]
    PasswordVerificationFailed,
    #[snafu(display("the persisted operator identity is invalid"))]
    InvalidPersistedIdentity,
    #[snafu(display("the session credential is invalid"))]
    InvalidSessionCredential,
    #[snafu(display("the persisted operator role is invalid"))]
    InvalidPersistedRole,
    #[snafu(display("the operator login name must not be empty"))]
    EmptyLoginName,
    #[snafu(display("the operator password confirmation does not match"))]
    PasswordMismatch,
    #[snafu(display("operator password entropy is unavailable"))]
    EntropyUnavailable,
    #[snafu(display("the operator password hashing parameters are invalid"))]
    InvalidHashingParameters,
    #[snafu(display("the operator password salt could not be encoded"))]
    SaltEncodingFailed,
    #[snafu(display("the operator password could not be hashed"))]
    PasswordHashingFailed,
}

#[cfg(test)]
mod tests;
