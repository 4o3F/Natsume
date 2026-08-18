use snafu::Snafu;
use uuid::Uuid;

mod account;
mod password;
mod session;

pub(crate) use self::account::{AccountFacts, create_first_admin, reset_operator_password};
#[cfg(test)]
pub(crate) use self::account::{create_first_admin_with_ids, reset_operator_password_with_ids};
#[cfg(test)]
pub(crate) use self::password::OperatorPassword;
pub(crate) use self::password::hash_password;
#[cfg(test)]
use self::password::{
    DUMMY_PASSWORD_PHC, PASSWORD_VERIFICATION_CONCURRENCY, PASSWORD_VERIFICATION_GATE,
    verify_password_once,
};
pub(crate) use self::session::{
    OperatorCredentials, SessionCredentialHex, SessionFacts, authenticate_session, sign_in,
    terminate_session,
};
#[cfg(test)]
use self::session::{SESSION_CREDENTIAL_LENGTH, decode_lower_hex};
#[cfg(test)]
pub(crate) use self::session::{
    SessionCredential, SessionCredentialHash, create_session, create_session_with_audit_id,
    terminate_session_with_audit_id,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperatorRole {
    Admin,
    Viewer,
}

impl OperatorRole {
    #[must_use]
    pub(crate) const fn as_persisted(self) -> &'static str {
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
    pub(crate) fn from_persisted(value: &str) -> Result<Self, OperatorError> {
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
    pub(crate) fn from_persisted(operator_id: &str, role: &str) -> Result<Self, OperatorError> {
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
    pub(crate) const fn role(self) -> OperatorRole {
        self.role
    }
}

/// Applies the closed two-role authorization policy.
///
/// # Errors
///
/// Returns [`OperatorError::AuthorizationDenied`] for a viewer.
pub(crate) const fn require_admin(role: OperatorRole) -> Result<(), OperatorError> {
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
pub(crate) mod tests;
