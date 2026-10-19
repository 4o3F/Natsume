use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    sync::Arc,
};

use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::db::{Database, PersistenceError, Transaction, TransactionError};

use super::{
    FINGERPRINT_VERSION, candidate_fingerprint,
    diff::{RedactedImportPreview, compute_diff},
    parse_roster,
    roster::{CandidateRoster, changed_passwords},
};

const IMPORT_CANDIDATE_TTL_MS: i64 = 1_800_000;
const PREVIEW_TOKEN_LENGTH: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CandidateExpiry {
    Valid,
    Expired,
}

pub(super) struct CandidateRecord {
    candidate_id: Uuid,
    expires_at_unix_ms: i64,
    preview_token_hash: [u8; 32],
    fingerprint_version: i32,
    candidate_fingerprint_sha256: [u8; 32],
    baseline_fingerprint_sha256: [u8; 32],
    diff: RedactedImportPreview,
    expiry: CandidateExpiry,
}

impl CandidateRecord {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        candidate_id: Uuid,
        expires_at_unix_ms: i64,
        preview_token_hash: [u8; 32],
        fingerprint_version: i32,
        candidate_fingerprint_sha256: [u8; 32],
        baseline_fingerprint_sha256: [u8; 32],
        diff: RedactedImportPreview,
        expiry: CandidateExpiry,
    ) -> Self {
        Self {
            candidate_id,
            expires_at_unix_ms,
            preview_token_hash,
            fingerprint_version,
            candidate_fingerprint_sha256,
            baseline_fingerprint_sha256,
            diff,
            expiry,
        }
    }

    pub(super) const fn candidate_id(&self) -> Uuid {
        self.candidate_id
    }

    pub(super) const fn expires_at_unix_ms(&self) -> i64 {
        self.expires_at_unix_ms
    }

    pub(super) const fn preview_token_hash(&self) -> &[u8; 32] {
        &self.preview_token_hash
    }

    pub(super) const fn fingerprint_version(&self) -> i32 {
        self.fingerprint_version
    }

    pub(super) const fn candidate_fingerprint_sha256(&self) -> &[u8; 32] {
        &self.candidate_fingerprint_sha256
    }

    pub(super) const fn baseline_fingerprint_sha256(&self) -> &[u8; 32] {
        &self.baseline_fingerprint_sha256
    }

    pub(super) const fn diff(&self) -> &RedactedImportPreview {
        &self.diff
    }

    pub(super) const fn expiry(&self) -> CandidateExpiry {
        self.expiry
    }

    fn into_pending(self) -> PendingImportCandidate {
        PendingImportCandidate {
            candidate_id: self.candidate_id,
            expires_at_unix_ms: self.expires_at_unix_ms,
            diff: self.diff,
        }
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
struct PreviewToken {
    bytes: [u8; PREVIEW_TOKEN_LENGTH],
}

impl PreviewToken {
    fn generate() -> Result<Self, ImportError> {
        let mut token = Self {
            bytes: [0; PREVIEW_TOKEN_LENGTH],
        };
        getrandom::fill(&mut token.bytes).map_err(|_| ImportError::EntropyUnavailable)?;
        Ok(token)
    }

    const fn as_bytes(&self) -> &[u8; PREVIEW_TOKEN_LENGTH] {
        &self.bytes
    }

    fn sha256(&self) -> [u8; 32] {
        Sha256::digest(self.bytes).into()
    }
}

pub(crate) struct CreatedImportCandidate {
    candidate_id: Uuid,
    preview_token: PreviewToken,
    expires_at_unix_ms: i64,
    diff: RedactedImportPreview,
}

impl CreatedImportCandidate {
    pub(crate) const fn candidate_id(&self) -> Uuid {
        self.candidate_id
    }

    pub(crate) const fn preview_token_bytes(&self) -> &[u8; PREVIEW_TOKEN_LENGTH] {
        self.preview_token.as_bytes()
    }

    pub(crate) const fn expires_at_unix_ms(&self) -> i64 {
        self.expires_at_unix_ms
    }

    pub(crate) const fn diff(&self) -> &RedactedImportPreview {
        &self.diff
    }
}

pub(crate) struct PendingImportCandidate {
    candidate_id: Uuid,
    expires_at_unix_ms: i64,
    diff: RedactedImportPreview,
}

impl PendingImportCandidate {
    pub(crate) const fn candidate_id(&self) -> Uuid {
        self.candidate_id
    }

    pub(crate) const fn expires_at_unix_ms(&self) -> i64 {
        self.expires_at_unix_ms
    }

    pub(crate) const fn diff(&self) -> &RedactedImportPreview {
        &self.diff
    }
}

pub(super) async fn create_import_candidate(
    database: &Database,
    vault: Arc<crate::vault::VaultSession>,
    raw_xlsx: &[u8],
) -> Result<CreatedImportCandidate, ImportError> {
    let parsed = parse_roster(raw_xlsx).await?;

    let preview_token = PreviewToken::generate()?;
    let preview_token_hash = preview_token.sha256();
    let candidate_id = Uuid::now_v7();

    database
        .write(move |transaction| -> Result<_, ImportError> {
            if let Some(existing) = super::db::pending_import_candidate::find(transaction)? {
                if existing.expiry() == CandidateExpiry::Valid {
                    return Err(ImportError::CandidatePending);
                }
                expire_candidate(transaction, &existing)?;
            }

            let baseline = super::db::query::read_baseline(transaction)?;
            let baseline_hash = baseline.fingerprint();
            let roster = CandidateRoster::prepare(&baseline, &parsed)?;
            let candidate_hash = candidate_fingerprint(&roster)?;
            let changed = changed_passwords(&baseline, &parsed, &vault)?;
            let diff = compute_diff(&baseline, &roster, changed);
            let now = super::db::pending_import_candidate::current_time_unix_ms(transaction)?;
            let expires_at_unix_ms = now
                .checked_add(IMPORT_CANDIDATE_TTL_MS)
                .ok_or(ImportError::PersistenceFailure)?;
            let candidate = CandidateRecord::new(
                candidate_id,
                expires_at_unix_ms,
                preview_token_hash,
                FINGERPRINT_VERSION,
                candidate_hash,
                baseline_hash,
                diff,
                CandidateExpiry::Valid,
            );

            if super::db::pending_import_candidate::insert(transaction, &candidate)? != 1 {
                return Err(ImportError::PersistenceFailure);
            }
            Ok(CreatedImportCandidate {
                candidate_id,
                preview_token,
                expires_at_unix_ms,
                diff: candidate.diff,
            })
        })
        .await
        .map_err(TransactionError::into_error)
}

pub(super) async fn read_pending_import_candidate(
    database: &Database,
) -> Result<Option<PendingImportCandidate>, ImportError> {
    database
        .write(move |transaction| -> Result<_, ImportError> {
            let Some(candidate) = super::db::pending_import_candidate::find(transaction)? else {
                return Ok(None);
            };
            if candidate.expiry() == CandidateExpiry::Expired {
                expire_candidate(transaction, &candidate)?;
                return Ok(None);
            }
            Ok(Some(candidate.into_pending()))
        })
        .await
        .map_err(TransactionError::into_error)
}

pub(super) async fn discard_import(
    database: &Database,
    candidate_id: Uuid,
) -> Result<(), ImportError> {
    let outcome = database
        .write(move |transaction| {
            let Some(candidate) = super::db::pending_import_candidate::find(transaction)? else {
                return Err(ImportError::CandidateUnavailable);
            };
            if candidate.candidate_id() != candidate_id {
                return Err(ImportError::CandidateUnavailable);
            }
            if candidate.expiry() == CandidateExpiry::Expired {
                expire_candidate(transaction, &candidate)?;
                return Ok(DiscardOutcome::Unavailable);
            }
            if super::db::pending_import_candidate::delete_exact(transaction, &candidate)? != 1 {
                return Err(ImportError::PersistenceFailure);
            }
            Ok(DiscardOutcome::Discarded)
        })
        .await
        .map_err(TransactionError::into_error)?;
    match outcome {
        DiscardOutcome::Discarded => Ok(()),
        DiscardOutcome::Unavailable => Err(ImportError::CandidateUnavailable),
    }
}

pub(super) fn expire_candidate(
    transaction: &mut Transaction<'_>,
    candidate: &CandidateRecord,
) -> Result<(), ImportError> {
    if super::db::pending_import_candidate::delete_exact(transaction, candidate)? != 1 {
        return Err(ImportError::PersistenceFailure);
    }
    Ok(())
}

enum DiscardOutcome {
    Discarded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImportError {
    InvalidWorkbook(natsume_roster::InputError),
    CandidateInvalid,
    CandidatePending,
    CandidateUnavailable,
    PreviewStale,
    SeatOccupied,
    EntropyUnavailable,
    VaultFailure,
    PersistenceFailure,
}

impl From<PersistenceError> for ImportError {
    fn from(error: PersistenceError) -> Self {
        match error {
            PersistenceError::InvalidPersistedData | PersistenceError::OperationFailed => {
                Self::PersistenceFailure
            }
        }
    }
}

impl Display for ImportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidWorkbook(_) | Self::CandidateInvalid => "the import candidate is invalid",
            Self::CandidatePending => "an import candidate is already pending",
            Self::CandidateUnavailable => "the import candidate is unavailable",
            Self::PreviewStale => "the import preview is stale",
            Self::SeatOccupied => "the import would remove an occupied seat",
            Self::EntropyUnavailable => "import candidate entropy is unavailable",
            Self::VaultFailure => "an import credential could not be accessed",
            Self::PersistenceFailure => "the import could not be persisted",
        })
    }
}

impl Error for ImportError {}
