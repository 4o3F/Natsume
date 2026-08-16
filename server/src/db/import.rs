use diesel::{QueryableByName, RunQueryDsl, sql_types::Text, sqlite::SqliteConnection};
use snafu::Snafu;
use uuid::Uuid;

use crate::application::import::{
    IMPORT_CANDIDATE_TTL_SECONDS, ImportError, RedactedImportPreview,
};

mod candidate;
mod commit;
mod diff;
mod discard;

pub(crate) use self::candidate::{
    audit_invalid_import_upload, audit_preview_token_mismatch, create_import_candidate,
    read_commit_candidate, read_pending_import_candidate,
};
pub(crate) use self::commit::commit_import;
pub(crate) use self::discard::discard_import;

#[cfg(test)]
use self::{
    candidate::{
        CandidateCreationRequest, audit_preview_token_mismatch_with_ids,
        create_import_candidate_with_ids,
    },
    commit::{CommitOutcome, CommitRequest, commit_import_with_ids},
};

pub(crate) struct CreatedCandidateFacts {
    pub(crate) candidate_id: Uuid,
    pub(crate) expires_at: String,
    pub(crate) baseline_configuration_revision: i64,
    pub(crate) baseline_binding_revision: i64,
    pub(crate) diff: RedactedImportPreview,
}

pub(crate) struct PendingCandidateFacts {
    pub(crate) candidate_id: Uuid,
    pub(crate) expires_at: String,
    pub(crate) baseline_configuration_revision: i64,
    pub(crate) baseline_binding_revision: i64,
    pub(crate) diff: RedactedImportPreview,
}

fn canonical_uuid_v7(value: &str) -> Result<Uuid, ImportStoreError> {
    let parsed = Uuid::parse_str(value).map_err(|_| ImportStoreError::InvalidPersistedFacts)?;
    if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != value {
        return Err(ImportStoreError::InvalidPersistedFacts);
    }
    Ok(parsed)
}

#[derive(QueryableByName)]
struct ExpiryRow {
    #[diesel(sql_type = Text)]
    expires_at: String,
}

fn import_expiry(connection: &mut SqliteConnection) -> Result<String, ImportStoreError> {
    let modifier = format!("+{IMPORT_CANDIDATE_TTL_SECONDS} seconds");
    diesel::sql_query("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?) AS expires_at")
        .bind::<Text, _>(modifier)
        .get_result::<ExpiryRow>(connection)
        .map(|row| row.expires_at)
        .map_err(|_| ImportStoreError::ExpiryCalculationFailed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
enum ImportStoreError {
    #[snafu(display("an import candidate is already pending"))]
    CandidatePending,
    #[snafu(display("the database connection could not be acquired"))]
    AcquireFailed,
    #[snafu(display("the import candidate transaction failed"))]
    TransactionFailed,
    #[snafu(display("the pending import candidate could not be read"))]
    PendingReadFailed,
    #[snafu(display("persisted import facts were invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("candidate import facts were invalid"))]
    InvalidCandidateFacts,
    #[snafu(display("the pending import candidate could not be deleted"))]
    CandidateDeleteFailed,
    #[snafu(display("the import payload could not be deleted"))]
    VaultDeleteFailed,
    #[snafu(display("the import vault record could not be read"))]
    VaultReadFailed,
    #[snafu(display("the revision counters could not be read"))]
    RevisionsReadFailed,
    #[snafu(display("the current contest facts could not be read"))]
    CurrentFactsReadFailed,
    #[snafu(display("the redacted import preview could not be serialized"))]
    PreviewSerializationFailed,
    #[snafu(display("the import payload could not be persisted"))]
    VaultInsertFailed,
    #[snafu(display("the import candidate expiry could not be calculated"))]
    ExpiryCalculationFailed,
    #[snafu(display("the import candidate could not be persisted"))]
    CandidateInsertFailed,
    #[snafu(display("the import audit event could not be persisted"))]
    AuditInsertFailed,
    #[snafu(display("the committed import mutation failed"))]
    MutationFailed,
    #[snafu(display("the committed import mutation changed concurrently"))]
    MutationConflict,
    #[snafu(display("an import revision could not be advanced"))]
    RevisionOverflow,
    #[snafu(display("an account credential revision could not be advanced"))]
    CredentialRevisionOverflow,
}

impl From<diesel::result::Error> for ImportStoreError {
    fn from(_source: diesel::result::Error) -> Self {
        Self::TransactionFailed
    }
}

impl From<ImportStoreError> for ImportError {
    fn from(source: ImportStoreError) -> Self {
        match source {
            ImportStoreError::CandidatePending => Self::CandidatePending,
            ImportStoreError::InvalidCandidateFacts => Self::CandidateInvalid,
            ImportStoreError::AcquireFailed
            | ImportStoreError::TransactionFailed
            | ImportStoreError::PendingReadFailed
            | ImportStoreError::InvalidPersistedFacts
            | ImportStoreError::CandidateDeleteFailed
            | ImportStoreError::VaultDeleteFailed
            | ImportStoreError::VaultReadFailed
            | ImportStoreError::RevisionsReadFailed
            | ImportStoreError::CurrentFactsReadFailed
            | ImportStoreError::PreviewSerializationFailed
            | ImportStoreError::VaultInsertFailed
            | ImportStoreError::ExpiryCalculationFailed
            | ImportStoreError::CandidateInsertFailed
            | ImportStoreError::AuditInsertFailed
            | ImportStoreError::MutationFailed
            | ImportStoreError::MutationConflict
            | ImportStoreError::RevisionOverflow
            | ImportStoreError::CredentialRevisionOverflow => Self::PersistenceFailure,
        }
    }
}

#[cfg(test)]
mod tests;
