use std::{net::IpAddr, time::Duration};

use sha2::{Digest as _, Sha256};
use snafu::{IntoError as _, OptionExt as _, ResultExt as _, Snafu, ensure};
use subtle::ConstantTimeEq as _;
use uuid::Uuid;

use super::{PhotoComponent, db};
use crate::db::{PersistenceError, Transaction, TransactionError};

const CODE_ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
const CODE_LIFETIME: i64 = 600_000;
const TOKEN_LIFETIME: i64 = 3_600_000;

#[derive(Debug, Snafu)]
pub(crate) enum EnrollmentError {
    #[snafu(display("Photo source name is invalid"))]
    InvalidName,
    #[snafu(display("Photo identifier is invalid"))]
    InvalidId,
    #[snafu(display("Photo pairing request is invalid"))]
    InvalidPairing,
    #[snafu(display("Photo signing public key is invalid"))]
    InvalidKey,
    #[snafu(display("Photo authentication failed"))]
    Authentication,
    #[snafu(display("Photo source does not exist"))]
    NotFound,
    #[snafu(display("Photo source transition conflicts with its current state"))]
    Conflict,
    #[snafu(display("Photo pairing rate limit exceeded"))]
    RateLimited,
    #[snafu(display("Photo enrollment entropy is unavailable"))]
    Entropy { source: getrandom::Error },
    #[snafu(display("Photo enrollment persistence failed"))]
    Persistence { source: PersistenceError },
}
impl From<PersistenceError> for EnrollmentError {
    fn from(source: PersistenceError) -> Self {
        PersistenceSnafu.into_error(source)
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceState {
    Enabled,
    Disabled,
    Revoked,
}
impl SourceState {
    fn wire(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Revoked => "revoked",
        }
    }
    pub(super) fn from_persisted(value: &str) -> Result<Self, PersistenceError> {
        match value {
            "enabled" => Ok(Self::Enabled),
            "disabled" => Ok(Self::Disabled),
            "revoked" => Ok(Self::Revoked),
            _ => Err(PersistenceError::invalid("photo source state")),
        }
    }
}

/// Completion may replay its saved receipt after expiry; initialization may not.
#[derive(Debug, Clone, Copy)]
pub(crate) enum CredentialUse {
    Initialization,
    Completion,
}

/// Replacing a pending binding must be an explicit administrator decision.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PendingEnrollment {
    Preserve,
    Replace,
}

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PhotoSource {
    pub source_id: String,
    pub name: String,
    pub state: SourceState,
    pub enrollment_confirmed: bool,
    pub initialization_completed: bool,
}
#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PhotoPairingCode {
    pub source_id: String,
    pub code: String,
    #[specta(type = specta_typescript::Number)]
    pub expires_at_unix_ms: i64,
}
// No Debug derive: this value contains a freshly issued credential.
#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PhotoEnrollmentIssued {
    pub source_id: String,
    pub enrollment_id: String,
    pub token: String,
    #[specta(type = specta_typescript::Number)]
    pub expires_at_unix_ms: i64,
}

#[derive(Clone)]
pub(super) struct Credential {
    pub source: String,
    pub enrollment: String,
    pub hash: [u8; 32],
}

pub(super) fn canonical_id(value: &str) -> Result<(), EnrollmentError> {
    ensure!(
        Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value),
        InvalidIdSnafu
    );
    Ok(())
}

pub(super) fn hash(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}
pub(super) fn credential(token: &str) -> Result<Credential, EnrollmentError> {
    let mut parts = token.split('.');
    let (Some("nph1"), Some(source), Some(enrollment), Some(secret), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return AuthenticationSnafu.fail();
    };
    ensure!(
        canonical_id(source).is_ok()
            && canonical_id(enrollment).is_ok()
            && secret.len() == 64
            && secret
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        AuthenticationSnafu
    );
    Ok(Credential {
        source: source.to_owned(),
        enrollment: enrollment.to_owned(),
        hash: hash(token.as_bytes()),
    })
}

/// Completion replay can authenticate an expired, completed credential, but may
/// only retrieve its already-recorded receipt. Download paths never use this mode.
pub(super) fn authorized(
    tx: &mut Transaction<'_>,
    credential: &Credential,
    purpose: CredentialUse,
) -> Result<db::SourceRecord, EnrollmentError> {
    let source = db::source(tx, &credential.source)?.context(AuthenticationSnafu)?;
    ensure!(
        matches!(source.state, SourceState::Enabled)
            && source.enrollment_id.as_deref() == Some(&credential.enrollment)
            && source
                .token_hash
                .as_deref()
                .is_some_and(|actual| bool::from(actual.ct_eq(&credential.hash))),
        AuthenticationSnafu
    );
    let completed = db::initialization_completed(tx, &source.id)?;
    let now = db::now(tx)?;
    ensure!(
        (matches!(purpose, CredentialUse::Completion) && completed)
            || (!completed && source.token_expires.is_some_and(|expires| expires > now)),
        AuthenticationSnafu
    );
    Ok(source)
}

fn projection(
    tx: &mut Transaction<'_>,
    source: db::SourceRecord,
) -> Result<PhotoSource, EnrollmentError> {
    Ok(PhotoSource {
        initialization_completed: db::initialization_completed(tx, &source.id)?,
        source_id: source.id,
        name: source.name,
        state: source.state,
        enrollment_confirmed: source.confirmed,
    })
}

impl PhotoComponent {
    /// Authenticate before adapters read mutation request bodies. Operations
    /// recheck the same credential in their own transaction before changing state.
    pub(crate) async fn check_token(
        &self,
        token: &str,
        purpose: CredentialUse,
    ) -> Result<(), EnrollmentError> {
        let credential = credential(token)?;
        self.database
            .read(move |tx| authorized(tx, &credential, purpose).map(|_| ()))
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn admit_pairing(&self, peer: IpAddr) -> Result<(), EnrollmentError> {
        let mut attempts = self.pairing_attempts.lock().await;
        let now = tokio::time::Instant::now();
        attempts.retain(|(time, _)| now.duration_since(*time) < Duration::from_mins(1));
        let peer = peer.to_canonical();
        ensure!(
            attempts.len() < 60 && attempts.iter().filter(|(_, ip)| *ip == peer).count() < 5,
            RateLimitedSnafu
        );
        attempts.push_back((now, peer));
        Ok(())
    }

    pub(crate) async fn create_source(&self, name: String) -> Result<PhotoSource, EnrollmentError> {
        ensure!(
            (1..=128).contains(&name.chars().count())
                && !name.trim().is_empty()
                && !name.chars().any(char::is_control),
            InvalidNameSnafu
        );
        let id = Uuid::now_v7().to_string();
        self.database
            .write(move |tx| {
                let now = db::now(tx)?;
                db::insert_source(tx, &id, &name, now)?;
                let source = db::source(tx, &id)?.context(NotFoundSnafu)?;
                projection(tx, source)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn list_sources(&self) -> Result<Vec<PhotoSource>, EnrollmentError> {
        self.database
            .read(|tx| {
                db::list(tx)?
                    .into_iter()
                    .map(|source| projection(tx, source))
                    .collect()
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn set_state(
        &self,
        id: String,
        state: SourceState,
    ) -> Result<(), EnrollmentError> {
        canonical_id(&id)?;
        self.database
            .write(move |tx| {
                let source = db::source(tx, &id)?.context(NotFoundSnafu)?;
                ensure!(
                    !matches!(source.state, SourceState::Revoked)
                        || matches!(state, SourceState::Revoked),
                    ConflictSnafu
                );
                db::update_state(tx, &id, state.wire())?;
                Ok(())
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn issue_code(
        &self,
        id: String,
        pending: PendingEnrollment,
    ) -> Result<PhotoPairingCode, EnrollmentError> {
        canonical_id(&id)?;
        let mut random = [0; 8];
        getrandom::fill(&mut random).context(EntropySnafu)?;
        let raw: String = random
            .iter()
            .map(|byte| char::from(CODE_ALPHABET[usize::from(byte & 31)]))
            .collect();
        let digest = hash(raw.as_bytes());
        self.database
            .write(move |tx| {
                let source = db::source(tx, &id)?.context(NotFoundSnafu)?;
                ensure!(
                    matches!(source.state, SourceState::Enabled)
                        && !db::initialization_completed(tx, &id)?
                        && (source.enrollment_id.is_none()
                            || matches!(pending, PendingEnrollment::Replace)),
                    ConflictSnafu
                );
                if matches!(pending, PendingEnrollment::Replace) {
                    db::discard_uncompleted_initialization(tx, &id)?;
                }
                let expires = db::now(tx)? + CODE_LIFETIME;
                db::issue_code(tx, &id, &digest, expires)?;
                Ok(PhotoPairingCode {
                    source_id: id,
                    code: format!("{}-{}", &raw[..4], &raw[4..]),
                    expires_at_unix_ms: expires,
                })
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn pair(
        &self,
        code: String,
        public_key: String,
        version: String,
    ) -> Result<PhotoEnrollmentIssued, EnrollmentError> {
        let raw = code.trim().replace('-', "").to_ascii_uppercase();
        ensure!(
            raw.len() == 8
                && raw.bytes().all(|byte| CODE_ALPHABET.contains(&byte))
                && semver::Version::parse(&version).is_ok(),
            InvalidPairingSnafu
        );
        let key: [u8; 32] = hex::decode(&public_key)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .context(InvalidKeySnafu)?;
        let verifying = ed25519_dalek::VerifyingKey::from_bytes(&key)
            .ok()
            .context(InvalidKeySnafu)?;
        ensure!(
            !verifying.is_weak() && hex::encode(key) == public_key,
            InvalidKeySnafu
        );
        let enrollment = Uuid::now_v7().to_string();
        let mut secret = [0; 32];
        getrandom::fill(&mut secret).context(EntropySnafu)?;
        let code_hash = hash(raw.as_bytes());
        self.database
            .write(move |tx| {
                let id = db::code_source(tx, &code_hash)?.context(AuthenticationSnafu)?;
                let source = db::source(tx, &id)?.context(AuthenticationSnafu)?;
                let now = db::now(tx)?;
                ensure!(
                    matches!(source.state, SourceState::Enabled)
                        && !source.confirmed
                        && source.enrollment_id.is_none()
                        && source
                            .code_hash
                            .as_deref()
                            .is_some_and(|actual| bool::from(actual.ct_eq(&code_hash)))
                        && source.code_expires.is_some_and(|expires| expires > now),
                    AuthenticationSnafu
                );
                let token = format!("nph1.{id}.{enrollment}.{}", hex::encode(secret));
                let expires = now + TOKEN_LIFETIME;
                db::bind(
                    tx,
                    &id,
                    &enrollment,
                    &public_key,
                    &hash(token.as_bytes()),
                    expires,
                )?;
                Ok(PhotoEnrollmentIssued {
                    source_id: id,
                    enrollment_id: enrollment,
                    token,
                    expires_at_unix_ms: expires,
                })
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn confirm(
        &self,
        token: &str,
        enrollment_id: String,
        public_key: String,
    ) -> Result<PhotoSource, EnrollmentError> {
        let credential = credential(token)?;
        self.database
            .write(move |tx| {
                let source = authorized(tx, &credential, CredentialUse::Initialization)?;
                ensure!(
                    source.enrollment_id.as_deref() == Some(&enrollment_id)
                        && source.public_key.as_deref() == Some(&public_key),
                    ConflictSnafu
                );
                db::confirm(tx, &source.id)?;
                let updated = db::source(tx, &source.id)?.context(NotFoundSnafu)?;
                projection(tx, updated)
            })
            .await
            .map_err(TransactionError::into_error)
    }
}
