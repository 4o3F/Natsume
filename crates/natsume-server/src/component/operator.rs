use snafu::Snafu;
use uuid::Uuid;

use crate::{
    db::{Database, PersistenceError, Transaction, TransactionError},
    journal::{
        Journal, JournalContext,
        event::{
            Event,
            payload::{AuthFailed, AuthSucceeded, LoggedOut},
            types::{AuthFailureClass, OperatorId},
        },
    },
};

mod account;
mod credentials;
mod db;
mod link;
mod management;
mod password;
mod session;

use self::account::AccountFacts;
use self::session::SessionFacts;
pub(crate) use self::{
    credentials::OperatorCredentials,
    management::{InvitationSummary, OperatorSummary, PasswordResetInspection},
    session::{SessionCredentialHex, SignedInSession},
};

/// Operator authentication and account authority with private persistence.
pub(crate) struct OperatorComponent {
    database: Database,
    /// Emit-only handle for authentication facts committed by this component.
    journal: Journal,
}

impl OperatorComponent {
    pub(crate) const fn new(database: Database, journal: Journal) -> Self {
        Self { database, journal }
    }

    /// Creates the first administrator within the caller's bootstrap transaction.
    pub(crate) fn create_first_admin(
        transaction: &mut Transaction<'_>,
        login_name: &str,
        password_hash: &str,
    ) -> Result<Uuid, OperatorError> {
        account::create_first_admin(transaction, login_name, password_hash)
    }

    pub(crate) fn is_initialized(transaction: &mut Transaction<'_>) -> Result<bool, OperatorError> {
        db::any_account_exists(transaction).map_err(OperatorError::from)
    }

    pub(crate) async fn reset_password_offline(
        database: &Database,
        login_name: &str,
        password_hash: &str,
    ) -> Result<(), OperatorError> {
        account::reset_operator_password(database, login_name, password_hash).await
    }

    pub(crate) async fn sign_in(
        &self,
        login_name: &str,
        submitted_password: String,
        context: JournalContext,
    ) -> Result<SignedInSession, OperatorError> {
        let result = session::sign_in(&self.database, login_name, submitted_password).await;
        match &result {
            Ok(session) => self.journal.emit(
                Event::AuthSucceeded(AuthSucceeded {}),
                context.with_actor(OperatorId::from_authenticated_operator(
                    session.identity().operator_id(),
                )),
            ),
            Err(error) => self.record_auth_failure(error, context),
        }
        result
    }

    pub(crate) async fn authenticate_session(
        &self,
        wire_credential: SessionCredentialHex,
    ) -> Result<OperatorIdentity, OperatorError> {
        session::authenticate_session(&self.database, wire_credential).await
    }

    pub(crate) async fn display_names(
        &self,
        ids: Vec<Uuid>,
    ) -> Result<std::collections::HashMap<Uuid, String>, PersistenceError> {
        self.database
            .read(move |transaction| db::management::display_names(transaction, &ids))
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn session_username(
        &self,
        identity: OperatorIdentity,
    ) -> Result<String, OperatorError> {
        self.database
            .read(move |transaction| {
                db::management::account_name(transaction, identity.operator_id())
            })
            .await
            .map_err(TransactionError::into_error)
            .map_err(|error| match error {
                OperatorError::OperatorNotFound => OperatorError::SessionAuthenticationFailed,
                error => error,
            })
    }

    pub(crate) async fn terminate_session(
        &self,
        wire_credential: SessionCredentialHex,
        context: JournalContext,
    ) -> Result<Option<OperatorIdentity>, OperatorError> {
        let identity = session::terminate_session(&self.database, wire_credential).await?;
        if let Some(identity) = identity {
            self.journal.emit(
                Event::LoggedOut(LoggedOut {}),
                context.with_actor(OperatorId::from_authenticated_operator(
                    identity.operator_id(),
                )),
            );
        }
        Ok(identity)
    }

    fn record_auth_failure(&self, error: &OperatorError, context: JournalContext) {
        let class = match error {
            OperatorError::SignInBusy => AuthFailureClass::RateLimited,
            OperatorError::AuthenticationFailed
            | OperatorError::SessionAuthenticationFailed
            | OperatorError::InvalidSessionCredential
            | OperatorError::CredentialsTooLong
            | OperatorError::EmptyLoginName => AuthFailureClass::InvalidCredentials,
            _ => AuthFailureClass::Infrastructure,
        };
        self.journal
            .emit(Event::AuthFailed(AuthFailed { class }), context);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperatorRole {
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
                .map_err(|source| OperatorError::InvalidPersistedIdentity { source })?,
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

#[derive(Debug, Snafu)]
pub(crate) enum OperatorError {
    #[snafu(display("operator authentication failed"))]
    AuthenticationFailed,
    #[snafu(display("operator sign-in capacity is exhausted"))]
    SignInBusy,
    #[snafu(display("operator credentials exceed the byte limits"))]
    CredentialsTooLong,
    #[snafu(display("operator session authentication failed"))]
    SessionAuthenticationFailed,
    #[snafu(display("expired operator session cleanup failed"))]
    SessionCleanup { source: PersistenceError },
    #[snafu(display("operator authorization was denied"))]
    AuthorizationDenied,
    #[snafu(display("operator persistence failed"))]
    PersistenceFailed { source: PersistenceError },
    #[snafu(display("the operator password task failed"))]
    PasswordTaskFailed {
        #[snafu(source(from(tokio::task::JoinError, crate::task::TaskFailure::from)))]
        source: crate::task::TaskFailure,
    },
    #[snafu(display("operator password verification failed"))]
    PasswordVerificationFailed,
    #[snafu(display("operator password verification failed"))]
    PasswordVerification {
        source: argon2::password_hash::Error,
    },
    #[snafu(display("the persisted operator identity is invalid"))]
    InvalidPersistedIdentity { source: uuid::Error },
    #[snafu(display("the session credential is invalid"))]
    InvalidSessionCredential,
    #[snafu(display("the persisted operator role is invalid"))]
    InvalidPersistedRole,
    #[snafu(display("the operator login name must not be empty"))]
    EmptyLoginName,
    #[snafu(display("the new operator login name must not have surrounding whitespace"))]
    InvalidNewLoginName,
    #[snafu(display("the new operator password does not satisfy the password policy"))]
    InvalidNewPassword,
    #[snafu(display("the operator password confirmation does not match"))]
    PasswordMismatch,
    #[snafu(display("operator password entropy is unavailable"))]
    EntropyUnavailable { source: getrandom::Error },
    #[snafu(display("the operator password hashing parameters are invalid"))]
    InvalidHashingParameters { source: argon2::Error },
    #[snafu(display("the operator password salt could not be encoded"))]
    SaltEncodingFailed {
        source: argon2::password_hash::phc::Error,
    },
    #[snafu(display("the operator password could not be hashed"))]
    PasswordHashingFailed {
        source: argon2::password_hash::Error,
    },
    #[snafu(display("the operator does not exist"))]
    OperatorNotFound,
    #[snafu(display("the operator login name is already in use"))]
    LoginNameConflict,
    #[snafu(display("the last administrator cannot be removed"))]
    LastAdmin,
    #[snafu(display("the operator link is invalid or unavailable"))]
    LinkUnavailable,
    #[snafu(display("the operator credential revision has changed"))]
    CredentialChanged,
}

impl From<PersistenceError> for OperatorError {
    fn from(source: PersistenceError) -> Self {
        Self::PersistenceFailed { source }
    }
}

#[cfg(test)]
pub(crate) mod tests;
