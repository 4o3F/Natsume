use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::super::{csv::CsvImportError, diff::RedactedImportPreview};

pub(crate) const IMPORT_CANDIDATE_TTL_SECONDS: i64 = 1_800;

pub(in crate::application::import) const PREVIEW_TOKEN_LENGTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CandidateRowFacts {
    pub(crate) seat_code: String,
    pub(crate) domjudge_username: String,
}

impl CandidateRowFacts {
    #[must_use]
    pub(crate) fn seat_code(&self) -> &str {
        &self.seat_code
    }

    #[must_use]
    pub(crate) fn domjudge_username(&self) -> &str {
        &self.domjudge_username
    }
}

pub(crate) struct SealedCommitRow {
    pub(crate) seat_code: String,
    pub(crate) domjudge_username: String,
    pub(crate) nonce: [u8; 24],
    pub(crate) ciphertext: Vec<u8>,
}

impl SealedCommitRow {
    #[must_use]
    pub(crate) fn seat_code(&self) -> &str {
        &self.seat_code
    }

    #[must_use]
    pub(crate) fn domjudge_username(&self) -> &str {
        &self.domjudge_username
    }

    #[must_use]
    pub(crate) const fn nonce(&self) -> &[u8; 24] {
        &self.nonce
    }

    #[must_use]
    pub(crate) fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateExpiry {
    Valid,
    Expired,
}

pub(crate) struct CandidateRecord {
    pending: PendingImportCandidate,
    preview_token_hash: [u8; 32],
    payload_vault_record_id: Uuid,
    expiry: CandidateExpiry,
}

impl CandidateRecord {
    pub(crate) fn new(
        pending: PendingImportCandidate,
        preview_token_hash: [u8; 32],
        payload_vault_record_id: Uuid,
        expiry: CandidateExpiry,
    ) -> Self {
        Self {
            pending,
            preview_token_hash,
            payload_vault_record_id,
            expiry,
        }
    }

    #[must_use]
    pub(crate) const fn candidate_id(&self) -> Uuid {
        self.pending.candidate_id
    }

    #[must_use]
    pub(crate) fn expires_at(&self) -> &str {
        &self.pending.expires_at
    }

    #[must_use]
    pub(crate) const fn baseline_configuration_revision(&self) -> i64 {
        self.pending.baseline_configuration_revision
    }

    #[must_use]
    pub(crate) const fn baseline_binding_revision(&self) -> i64 {
        self.pending.baseline_binding_revision
    }

    #[must_use]
    pub(crate) const fn diff(&self) -> &RedactedImportPreview {
        &self.pending.diff
    }

    #[must_use]
    pub(crate) const fn preview_token_hash(&self) -> &[u8; 32] {
        &self.preview_token_hash
    }

    #[must_use]
    pub(crate) const fn payload_vault_record_id(&self) -> Uuid {
        self.payload_vault_record_id
    }

    #[must_use]
    pub(crate) const fn expiry(&self) -> CandidateExpiry {
        self.expiry
    }

    pub(super) fn into_pending(self) -> PendingImportCandidate {
        self.pending
    }

    pub(super) fn into_created(self, preview_token: PreviewToken) -> CreatedImportCandidate {
        CreatedImportCandidate {
            candidate_id: self.pending.candidate_id,
            preview_token,
            expires_at: self.pending.expires_at,
            baseline_configuration_revision: self.pending.baseline_configuration_revision,
            baseline_binding_revision: self.pending.baseline_binding_revision,
            diff: self.pending.diff,
        }
    }
}

pub(crate) struct ImportPayloadFacts {
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

impl ImportPayloadFacts {
    pub(crate) fn new(nonce: Vec<u8>, ciphertext: Vec<u8>) -> Self {
        Self { nonce, ciphertext }
    }
}

pub(crate) struct CommitCandidatePayload {
    candidate: CandidateRecord,
    payload: ImportPayloadFacts,
}

impl CommitCandidatePayload {
    pub(super) fn new(candidate: CandidateRecord, payload: ImportPayloadFacts) -> Self {
        Self { candidate, payload }
    }

    #[must_use]
    pub(in crate::application::import) const fn candidate_id(&self) -> Uuid {
        self.candidate.candidate_id()
    }

    #[must_use]
    pub(in crate::application::import) const fn preview_token_hash(&self) -> &[u8; 32] {
        self.candidate.preview_token_hash()
    }

    #[must_use]
    pub(in crate::application::import) const fn payload_vault_record_id(&self) -> Uuid {
        self.candidate.payload_vault_record_id()
    }

    #[must_use]
    pub(in crate::application::import) fn nonce(&self) -> &[u8] {
        &self.payload.nonce
    }

    #[must_use]
    pub(in crate::application::import) fn ciphertext(&self) -> &[u8] {
        &self.payload.ciphertext
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct PreviewToken {
    pub(in crate::application::import) bytes: [u8; PREVIEW_TOKEN_LENGTH],
}

impl PreviewToken {
    pub(in crate::application::import) fn generate() -> Result<Self, ImportError> {
        let mut token = Self {
            bytes: [0_u8; PREVIEW_TOKEN_LENGTH],
        };
        getrandom::fill(&mut token.bytes).map_err(|_| ImportError::EntropyUnavailable)?;
        Ok(token)
    }

    pub(in crate::application::import) fn sha256(&self) -> [u8; 32] {
        Sha256::digest(self.bytes.as_slice()).into()
    }

    pub(crate) fn from_bytes(bytes: [u8; PREVIEW_TOKEN_LENGTH]) -> Self {
        Self { bytes }
    }

    #[must_use]
    pub(crate) const fn as_bytes(&self) -> &[u8; PREVIEW_TOKEN_LENGTH] {
        &self.bytes
    }
}

pub(crate) struct CreatedImportCandidate {
    pub(in crate::application::import) candidate_id: Uuid,
    pub(in crate::application::import) preview_token: PreviewToken,
    pub(in crate::application::import) expires_at: String,
    pub(in crate::application::import) baseline_configuration_revision: i64,
    pub(in crate::application::import) baseline_binding_revision: i64,
    pub(in crate::application::import) diff: RedactedImportPreview,
}

pub(crate) struct PendingImportCandidate {
    pub(in crate::application::import) candidate_id: Uuid,
    pub(in crate::application::import) expires_at: String,
    pub(in crate::application::import) baseline_configuration_revision: i64,
    pub(in crate::application::import) baseline_binding_revision: i64,
    pub(in crate::application::import) diff: RedactedImportPreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommittedImportFacts {
    pub(in crate::application::import) configuration_revision: i64,
    pub(in crate::application::import) binding_revision: i64,
}

impl CommittedImportFacts {
    pub(crate) const fn new(configuration_revision: i64, binding_revision: i64) -> Self {
        Self {
            configuration_revision,
            binding_revision,
        }
    }

    #[must_use]
    pub(crate) const fn configuration_revision(self) -> i64 {
        self.configuration_revision
    }

    #[must_use]
    pub(crate) const fn binding_revision(self) -> i64 {
        self.binding_revision
    }
}

impl CreatedImportCandidate {
    #[must_use]
    pub(crate) const fn candidate_id(&self) -> Uuid {
        self.candidate_id
    }

    #[must_use]
    pub(crate) const fn preview_token(&self) -> &PreviewToken {
        &self.preview_token
    }

    #[must_use]
    pub(crate) fn expires_at(&self) -> &str {
        &self.expires_at
    }

    #[must_use]
    pub(crate) const fn baseline_configuration_revision(&self) -> i64 {
        self.baseline_configuration_revision
    }

    #[must_use]
    pub(crate) const fn baseline_binding_revision(&self) -> i64 {
        self.baseline_binding_revision
    }

    #[must_use]
    pub(crate) const fn diff(&self) -> &RedactedImportPreview {
        &self.diff
    }
}

impl PendingImportCandidate {
    pub(crate) fn new(
        candidate_id: Uuid,
        expires_at: String,
        baseline_configuration_revision: i64,
        baseline_binding_revision: i64,
        diff: RedactedImportPreview,
    ) -> Self {
        Self {
            candidate_id,
            expires_at,
            baseline_configuration_revision,
            baseline_binding_revision,
            diff,
        }
    }

    #[must_use]
    pub(crate) const fn candidate_id(&self) -> Uuid {
        self.candidate_id
    }

    #[must_use]
    pub(crate) fn expires_at(&self) -> &str {
        &self.expires_at
    }

    #[must_use]
    pub(crate) const fn baseline_configuration_revision(&self) -> i64 {
        self.baseline_configuration_revision
    }

    #[must_use]
    pub(crate) const fn baseline_binding_revision(&self) -> i64 {
        self.baseline_binding_revision
    }

    #[must_use]
    pub(crate) const fn diff(&self) -> &RedactedImportPreview {
        &self.diff
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportError {
    InvalidCsv(CsvImportError),
    CandidateInvalid,
    CandidatePending,
    CandidateUnavailable,
    PreviewStale,
    EntropyUnavailable,
    VaultFailure,
    PersistenceFailure,
}

impl Display for ImportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidCsv(_) | Self::CandidateInvalid => "the import candidate is invalid",
            Self::CandidatePending => "an import candidate is already pending",
            Self::CandidateUnavailable => "the import candidate is unavailable",
            Self::PreviewStale => "the import preview is stale",
            Self::EntropyUnavailable => "import candidate entropy is unavailable",
            Self::VaultFailure => "the import payload could not be staged",
            Self::PersistenceFailure => "the import candidate could not be persisted",
        })
    }
}

impl Error for ImportError {}
