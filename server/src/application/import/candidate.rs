use std::{
    collections::HashSet,
    error::Error,
    fmt::{self, Display, Formatter},
    path::Path,
};

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{
    audit::CorrelationId,
    db::{self, Database},
    vault,
};

use super::{
    csv::{
        ACCOUNT_USERNAME_LENGTH_LIMIT, CsvImportError, ImportRow, MAX_IMPORT_ROWS,
        PASSWORD_LENGTH_LIMIT, SEAT_CODE_LENGTH_LIMIT, parse_csv,
    },
    diff::RedactedImportPreview,
};

pub(crate) const IMPORT_CANDIDATE_TTL_SECONDS: i64 = 1_800;

pub(super) const PREVIEW_TOKEN_LENGTH: usize = 32;

pub(crate) struct CandidateRowFacts {
    pub(crate) seat_code: String,
    pub(crate) domjudge_username: String,
}

pub(crate) struct SealedCommitRow {
    pub(crate) seat_code: String,
    pub(crate) domjudge_username: String,
    pub(crate) nonce: [u8; 24],
    pub(crate) ciphertext: Vec<u8>,
}

pub(crate) struct CommitCandidatePayload {
    pub(crate) candidate_id: Uuid,
    pub(crate) preview_token_hash: [u8; 32],
    pub(crate) payload_vault_record_id: Uuid,
    pub(crate) nonce: Vec<u8>,
    pub(crate) ciphertext: Vec<u8>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct PreviewToken {
    pub(super) bytes: [u8; PREVIEW_TOKEN_LENGTH],
}

impl PreviewToken {
    pub(super) fn generate() -> Result<Self, ImportError> {
        let mut token = Self {
            bytes: [0_u8; PREVIEW_TOKEN_LENGTH],
        };
        getrandom::fill(&mut token.bytes).map_err(|_| ImportError::EntropyUnavailable)?;
        Ok(token)
    }

    pub(super) fn sha256(&self) -> [u8; 32] {
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
    pub(super) candidate_id: Uuid,
    pub(super) preview_token: PreviewToken,
    pub(super) expires_at: String,
    pub(super) baseline_configuration_revision: i64,
    pub(super) baseline_binding_revision: i64,
    pub(super) diff: RedactedImportPreview,
}

pub(crate) struct PendingImportCandidate {
    pub(super) candidate_id: Uuid,
    pub(super) expires_at: String,
    pub(super) baseline_configuration_revision: i64,
    pub(super) baseline_binding_revision: i64,
    pub(super) diff: RedactedImportPreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommittedImportFacts {
    pub(super) configuration_revision: i64,
    pub(super) binding_revision: i64,
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

/// Strictly parses, encrypts, diffs, and atomically persists one import candidate.
///
/// # Errors
///
/// Returns a typed [`ImportError`] for invalid input, a live singleton candidate,
/// entropy failure, vault failure, or persistence failure.
pub(crate) async fn create_import_candidate(
    database: &Database,
    master_key_path: &Path,
    raw_csv: &[u8],
    correlation_id: CorrelationId,
) -> Result<CreatedImportCandidate, ImportError> {
    let parsed = parse_csv(raw_csv).map_err(ImportError::InvalidCsv)?;
    let preview_token = PreviewToken::generate()?;
    let preview_token_hash = preview_token.sha256();
    let candidate_rows = parsed.candidate_rows();
    let (nonce, ciphertext) = vault::seal(master_key_path, parsed.staging_plaintext())
        .map_err(|_| ImportError::VaultFailure)?;
    drop(parsed);

    let created = db::import::create_import_candidate(
        database,
        candidate_rows,
        preview_token_hash,
        nonce,
        ciphertext,
        correlation_id,
    )
    .await?;

    Ok(CreatedImportCandidate {
        candidate_id: created.candidate_id,
        preview_token,
        expires_at: created.expires_at,
        baseline_configuration_revision: created.baseline_configuration_revision,
        baseline_binding_revision: created.baseline_binding_revision,
        diff: created.diff,
    })
}

pub(crate) async fn audit_invalid_import_upload(
    database: &Database,
    correlation_id: CorrelationId,
) -> Result<(), ImportError> {
    db::import::audit_invalid_import_upload(database, correlation_id).await
}

pub(crate) async fn read_pending_import_candidate(
    database: &Database,
    correlation_id: CorrelationId,
) -> Result<Option<PendingImportCandidate>, ImportError> {
    db::import::read_pending_import_candidate(database, correlation_id)
        .await
        .map(|pending| {
            pending.map(|pending| PendingImportCandidate {
                candidate_id: pending.candidate_id,
                expires_at: pending.expires_at,
                baseline_configuration_revision: pending.baseline_configuration_revision,
                baseline_binding_revision: pending.baseline_binding_revision,
                diff: pending.diff,
            })
        })
}

pub(crate) async fn commit_import(
    database: &Database,
    master_key_path: &Path,
    import_id: Uuid,
    presented_token: &PreviewToken,
    correlation_id: CorrelationId,
) -> Result<CommittedImportFacts, ImportError> {
    let payload = db::import::read_commit_candidate(database, import_id, correlation_id).await?;
    let presented_token_hash = presented_token.sha256();
    if !bool::from(
        presented_token_hash
            .as_slice()
            .ct_eq(payload.preview_token_hash.as_slice()),
    ) {
        db::import::audit_preview_token_mismatch(
            database,
            payload.candidate_id,
            payload.preview_token_hash,
            correlation_id,
        )
        .await?;
        return Err(ImportError::CandidateUnavailable);
    }

    let vault_session = vault::load(master_key_path).map_err(|_| ImportError::VaultFailure)?;
    let plaintext = vault_session
        .open(&payload.nonce, &payload.ciphertext)
        .map_err(|_| ImportError::VaultFailure)?;
    let rows = decode_staging_rows(&plaintext)?;
    drop(plaintext);
    let sealed_rows = seal_commit_rows(&vault_session, &rows)?;
    drop(rows);
    drop(vault_session);

    db::import::commit_import(
        database,
        payload.candidate_id,
        payload.preview_token_hash,
        payload.payload_vault_record_id,
        sealed_rows,
        correlation_id,
    )
    .await
}

pub(crate) async fn discard_import(
    database: &Database,
    import_id: Uuid,
    correlation_id: CorrelationId,
) -> Result<(), ImportError> {
    db::import::discard_import(database, import_id, correlation_id).await
}

pub(super) fn decode_staging_rows(plaintext: &[u8]) -> Result<Vec<ImportRow>, ImportError> {
    let rows = serde_json::from_slice::<Vec<ImportRow>>(plaintext)
        .map_err(|_| ImportError::PersistenceFailure)?;
    validate_staging_rows(&rows)?;
    Ok(rows)
}

pub(super) fn validate_staging_rows(rows: &[ImportRow]) -> Result<(), ImportError> {
    if rows.is_empty() || rows.len() > MAX_IMPORT_ROWS {
        return Err(ImportError::PersistenceFailure);
    }
    let mut seat_codes = HashSet::with_capacity(rows.len());
    let mut account_usernames = HashSet::with_capacity(rows.len());
    for row in rows {
        if !valid_staged_field(&row.seat_code, SEAT_CODE_LENGTH_LIMIT)
            || !valid_staged_field(&row.domjudge_username, ACCOUNT_USERNAME_LENGTH_LIMIT)
            || !valid_staged_field(&row.password, PASSWORD_LENGTH_LIMIT)
            || !seat_codes.insert(row.seat_code.as_str())
            || !account_usernames.insert(row.domjudge_username.as_str())
        {
            return Err(ImportError::PersistenceFailure);
        }
    }
    Ok(())
}

pub(super) fn valid_staged_field(value: &str, length_limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= length_limit
        && !value.contains(',')
        && !value.chars().any(char::is_control)
}

pub(super) fn seal_commit_rows(
    vault_session: &vault::VaultSession,
    rows: &[ImportRow],
) -> Result<Vec<SealedCommitRow>, ImportError> {
    let mut sealed_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let (nonce, ciphertext) = vault_session
            .seal(row.password.as_bytes())
            .map_err(|_| ImportError::VaultFailure)?;
        sealed_rows.push(SealedCommitRow {
            seat_code: row.seat_code.clone(),
            domjudge_username: row.domjudge_username.clone(),
            nonce,
            ciphertext,
        });
    }
    Ok(sealed_rows)
}
