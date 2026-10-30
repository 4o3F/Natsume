use std::{net::IpAddr, time::Instant};

use natsume_photo_protocol::{EnrollmentId, SourceId};
use natsume_photo_protocol::{PhotoConfirmation, PhotoPairRequest};
use sha2::{Digest as _, Sha256};
use snafu::{IntoError as _, OptionExt as _, ResultExt as _, Snafu, ensure};
use subtle::ConstantTimeEq as _;

use super::{PhotoComponent, db};
use crate::db::{PersistenceError, Transaction, TransactionError};

const CODE_LIFETIME: i64 = 600_000;
const TOKEN_LIFETIME: i64 = 3_600_000;

#[derive(Debug, Snafu)]
pub(crate) enum EnrollmentError {
    #[snafu(display("Photo pairing state is unavailable"))]
    StatePoisoned { source: std::sync::PoisonError<()> },
    #[snafu(display("Photo source name is invalid"))]
    InvalidName,
    #[snafu(display("Photo pairing request is invalid"))]
    InvalidPairing,
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
    Persistence { source: db::PersistenceError },
}
impl From<PersistenceError> for EnrollmentError {
    fn from(source: PersistenceError) -> Self {
        PersistenceSnafu.into_error(source.into())
    }
}

impl From<db::PersistenceError> for EnrollmentError {
    fn from(source: db::PersistenceError) -> Self {
        PersistenceSnafu.into_error(source)
    }
}
pub(crate) use natsume_photo_protocol::{PhotoSource, SourceState};
fn source_state_wire(state: SourceState) -> &'static str {
    match state {
        SourceState::Enabled => "enabled",
        SourceState::Disabled => "disabled",
        SourceState::Revoked => "revoked",
    }
}
pub(super) fn source_state_from_persisted(value: &str) -> Result<SourceState, PersistenceError> {
    match value {
        "enabled" => Ok(SourceState::Enabled),
        "disabled" => Ok(SourceState::Disabled),
        "revoked" => Ok(SourceState::Revoked),
        _ => Err(PersistenceError::invalid("photo source state")),
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
pub(crate) struct PhotoPairingCode {
    pub source_id: SourceId,
    pub code: String,
    #[specta(type = specta_typescript::Number)]
    pub expires_at_unix_ms: i64,
}
// No Debug derive: this value contains a freshly issued credential.
pub(crate) use natsume_photo_protocol::PhotoEnrollmentIssued;

#[derive(Clone)]
pub(super) struct Credential {
    pub source: SourceId,
    pub enrollment: EnrollmentId,
    pub hash: [u8; 32],
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
    // Authentication failures deliberately disclose neither token layout nor rejected values.
    let source = source.parse().ok().context(AuthenticationSnafu)?;
    let enrollment = enrollment.parse().ok().context(AuthenticationSnafu)?;
    ensure!(
        secret.len() == 64
            && secret
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        AuthenticationSnafu
    );
    Ok(Credential {
        source,
        enrollment,
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
            && source.enrollment_id.as_ref() == Some(&credential.enrollment)
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

    pub(crate) fn admit_pairing(&self, peer: IpAddr) -> Result<(), EnrollmentError> {
        let mut attempts = self.pairing_attempts.lock().map_err(|source| {
            drop(source.into_inner());
            StatePoisonedSnafu.into_error(std::sync::PoisonError::new(()))
        })?;
        ensure!(attempts.admit(peer, Instant::now()), RateLimitedSnafu);
        Ok(())
    }

    pub(crate) async fn create_source(&self, name: String) -> Result<PhotoSource, EnrollmentError> {
        ensure!(
            (1..=128).contains(&name.chars().count())
                && !name.trim().is_empty()
                && !name.chars().any(char::is_control),
            InvalidNameSnafu
        );
        let id = SourceId::generate();
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
        id: SourceId,
        state: SourceState,
    ) -> Result<(), EnrollmentError> {
        self.database
            .write(move |tx| {
                let source = db::source(tx, &id)?.context(NotFoundSnafu)?;
                ensure!(
                    !matches!(source.state, SourceState::Revoked)
                        || matches!(state, SourceState::Revoked),
                    ConflictSnafu
                );
                db::update_state(tx, &id, source_state_wire(state))?;
                Ok(())
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn issue_code(
        &self,
        id: SourceId,
        pending: PendingEnrollment,
    ) -> Result<PhotoPairingCode, EnrollmentError> {
        let mut random = [0; 8];
        getrandom::fill(&mut random).context(EntropySnafu)?;
        let raw = super::super::pairing::code(random);
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
        request: PhotoPairRequest,
    ) -> Result<PhotoEnrollmentIssued, EnrollmentError> {
        let PhotoPairRequest {
            code,
            signing_public_key: public_key,
            client_version: version,
        } = request;
        let raw = super::super::pairing::normalize(&code).context(InvalidPairingSnafu)?;
        ensure!(
            semver::Version::parse(&version).is_ok(),
            InvalidPairingSnafu
        );
        let enrollment = EnrollmentId::generate();
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
        request: PhotoConfirmation,
    ) -> Result<PhotoSource, EnrollmentError> {
        let credential = credential(token)?;
        self.database
            .write(move |tx| {
                let source = authorized(tx, &credential, CredentialUse::Initialization)?;
                ensure!(
                    source.enrollment_id.as_ref() == Some(&request.enrollment_id)
                        && source.public_key.as_ref() == Some(&request.signing_public_key),
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
