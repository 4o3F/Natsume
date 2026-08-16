use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{
        Error as PasswordHashError, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
    },
};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use snafu::Snafu;
use tokio::sync::Semaphore;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    audit::CorrelationId,
    db::{self, Database},
};

const ARGON2_MEMORY_COST_KIB: u32 = 19_456;
const ARGON2_TIME_COST: u32 = 2;
const ARGON2_PARALLELISM: u32 = 1;
const PASSWORD_SALT_LENGTH: usize = 16;
const SESSION_CREDENTIAL_LENGTH: usize = 32;
const SESSION_CREDENTIAL_HEX_LENGTH: usize = SESSION_CREDENTIAL_LENGTH * 2;
// The persisted hash width is the SHA-256 digest width, fixed independently of
// the credential width by `CHECK (length(session_credential_hash) = 32)`.
const SESSION_CREDENTIAL_HASH_LENGTH: usize = 32;
const DUMMY_PASSWORD_PHC: &str = "$argon2id$v=19$m=19456,t=2,p=1$\
    bmF0c3VtZS1kdW1teS1zbA$KQCQGYQS75NixY7KaNGTRwtboqGCDKN5SXQjAFrx+7w";
// One in-flight verification holds `ARGON2_MEMORY_COST_KIB` of memory, and the
// unauthenticated sign-in path always performs one, so without a bound the
// blocking pool lets anonymous callers allocate hundreds at once. Both inputs
// are frozen: the Argon2 memory cost above and the roughly three operator
// browsers a site runs. Device Enrollment and Device WSS never verify a
// password, so fleet size cannot raise this bound.
const PASSWORD_VERIFICATION_CONCURRENCY: usize = 4;

static PASSWORD_VERIFICATION_GATE: Semaphore =
    Semaphore::const_new(PASSWORD_VERIFICATION_CONCURRENCY);
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

/// The operator-account facts exactly as persisted, still unvalidated. The
/// database adapter constructs them and [`OperatorIdentity::from_persisted`] is
/// the validation that turns them into the closed domain type.
pub(crate) struct AccountFacts {
    pub(crate) operator_id: String,
    pub(crate) role: String,
    pub(crate) password_hash: String,
}

/// The operator-session facts exactly as persisted, still unvalidated.
pub(crate) struct SessionFacts {
    pub(crate) operator_id: String,
    pub(crate) role: String,
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
    pub(crate) const fn role(self) -> OperatorRole {
        self.role
    }
}

pub(crate) struct OperatorPassword(SecretString);

impl OperatorPassword {
    fn new(value: String) -> Self {
        Self(value.into())
    }

    fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

pub(crate) struct SessionCredential(Zeroizing<[u8; SESSION_CREDENTIAL_LENGTH]>);

impl SessionCredential {
    fn generate() -> Result<Self, OperatorError> {
        let mut bytes = Zeroizing::new([0_u8; SESSION_CREDENTIAL_LENGTH]);
        getrandom::fill(&mut *bytes).map_err(|_| OperatorError::EntropyUnavailable)?;
        Ok(Self(bytes))
    }

    fn from_wire(wire: &SessionCredentialHex) -> Result<Self, OperatorError> {
        decode_lower_hex(wire.expose())
    }

    pub(crate) fn to_wire(&self) -> SessionCredentialHex {
        SessionCredentialHex(encode_lower_hex(&self.0))
    }

    fn sha256(&self) -> SessionCredentialHash {
        SessionCredentialHash(Zeroizing::new(Sha256::digest(self.0.as_slice()).into()))
    }
}

pub(crate) struct SessionCredentialHex(SecretString);

impl SessionCredentialHex {
    pub(crate) fn new(value: String) -> Self {
        Self(value.into())
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

pub(crate) struct SessionCredentialHash(Zeroizing<[u8; SESSION_CREDENTIAL_HASH_LENGTH]>);

impl SessionCredentialHash {
    pub(crate) fn as_bytes(&self) -> &[u8; SESSION_CREDENTIAL_HASH_LENGTH] {
        &self.0
    }
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

pub(crate) struct OperatorCredentials {
    login_name: String,
    password: OperatorPassword,
}

impl OperatorCredentials {
    /// Validates non-interactive operator credential input.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`OperatorError`] when the login name is empty or the
    /// two passwords differ.
    pub(crate) fn new(
        login_name: String,
        password: String,
        password_confirmation: String,
    ) -> Result<Self, OperatorError> {
        let password = OperatorPassword::new(password);
        let password_confirmation = OperatorPassword::new(password_confirmation);
        if login_name.is_empty() {
            return Err(OperatorError::EmptyLoginName);
        }
        if password.expose() != password_confirmation.expose() {
            return Err(OperatorError::PasswordMismatch);
        }
        Ok(Self {
            login_name,
            password,
        })
    }

    pub(crate) fn login_name(&self) -> &str {
        &self.login_name
    }

    pub(crate) fn password(&self) -> &OperatorPassword {
        &self.password
    }
}

/// Hashes an operator password with the frozen Argon2id profile.
///
/// # Errors
///
/// Returns a redacted [`OperatorError`] if entropy, parameter construction,
/// salt encoding, or password hashing fails.
pub(crate) fn hash_password(password: &OperatorPassword) -> Result<String, OperatorError> {
    let params = Params::new(
        ARGON2_MEMORY_COST_KIB,
        ARGON2_TIME_COST,
        ARGON2_PARALLELISM,
        None,
    )
    .map_err(|_| OperatorError::InvalidHashingParameters)?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut salt_bytes = [0_u8; PASSWORD_SALT_LENGTH];
    getrandom::fill(&mut salt_bytes).map_err(|_| OperatorError::EntropyUnavailable)?;
    let salt =
        SaltString::encode_b64(&salt_bytes).map_err(|_| OperatorError::SaltEncodingFailed)?;
    argon2
        .hash_password(password.expose().as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| OperatorError::PasswordHashingFailed)
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
    let account = db::operator::read_account(database, login_name).await?;
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

    let identity = OperatorIdentity::from_persisted(&account.operator_id, &account.role)
        .map_err(|_| OperatorError::PersistenceFailed)?;
    let credential = SessionCredential::generate()?;
    let credential_hash = credential.sha256();
    db::operator::create_session(database, &credential_hash, identity, correlation_id).await?;

    Ok(SignedInSession {
        identity,
        credential,
    })
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
    let Some(facts) =
        db::operator::read_session(database, &credential_hash, correlation_id).await?
    else {
        return Err(OperatorError::SessionAuthenticationFailed);
    };
    OperatorIdentity::from_persisted(&facts.operator_id, &facts.role)
        .map_err(|_| OperatorError::PersistenceFailed)
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
    db::operator::terminate_session(database, &credential_hash, correlation_id).await
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

fn verify_password_once(
    password: &OperatorPassword,
    candidate_phc: &str,
) -> Result<bool, OperatorError> {
    let (parsed, persisted_profile_valid) =
        if let Some(parsed) = parse_frozen_password_hash(candidate_phc) {
            (parsed, true)
        } else {
            let dummy = parse_frozen_password_hash(DUMMY_PASSWORD_PHC)
                .ok_or(OperatorError::PasswordVerificationFailed)?;
            (dummy, false)
        };
    let verifier = frozen_argon2()?;

    let verification_succeeded =
        match verifier.verify_password(password.expose().as_bytes(), &parsed) {
            Ok(()) => true,
            Err(PasswordHashError::Password) => false,
            Err(_) => return Err(OperatorError::PasswordVerificationFailed),
        };

    Ok(persisted_profile_valid && verification_succeeded)
}

fn parse_frozen_password_hash(value: &str) -> Option<PasswordHash<'_>> {
    let parsed = PasswordHash::new(value).ok()?;
    if parsed.algorithm.as_str() != "argon2id"
        || parsed.version != Some(19)
        || parsed.params.iter().count() != 3
        || parsed.params.get_decimal("m") != Some(19_456)
        || parsed.params.get_decimal("t") != Some(2)
        || parsed.params.get_decimal("p") != Some(1)
    {
        return None;
    }
    let salt = parsed.salt?;
    let mut salt_bytes = [0_u8; PASSWORD_SALT_LENGTH];
    if salt.decode_b64(&mut salt_bytes).ok()?.len() != PASSWORD_SALT_LENGTH {
        return None;
    }
    Some(parsed)
}

fn frozen_argon2() -> Result<Argon2<'static>, OperatorError> {
    let params = Params::new(
        ARGON2_MEMORY_COST_KIB,
        ARGON2_TIME_COST,
        ARGON2_PARALLELISM,
        None,
    )
    .map_err(|_| OperatorError::InvalidHashingParameters)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

fn encode_lower_hex(bytes: &[u8; SESSION_CREDENTIAL_LENGTH]) -> SecretString {
    hex::encode(bytes).into()
}

fn decode_lower_hex(value: &str) -> Result<SessionCredential, OperatorError> {
    if value.len() != SESSION_CREDENTIAL_HEX_LENGTH
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OperatorError::InvalidSessionCredential);
    }
    let mut bytes = Zeroizing::new([0_u8; SESSION_CREDENTIAL_LENGTH]);
    hex::decode_to_slice(value, &mut *bytes)
        .map_err(|_| OperatorError::InvalidSessionCredential)?;
    Ok(SessionCredential(bytes))
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
