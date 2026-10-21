//! Durable `DOMjudge` admission, separate from Device Control and physical printing.
mod db;
mod pdf;
mod storage;
#[cfg(test)]
pub(crate) mod tests;

use std::path::PathBuf;

use sha2::{Digest as _, Sha256};
use snafu::Snafu;
use subtle::ConstantTimeEq as _;
use uuid::Uuid;

use super::operator::OperatorIdentity;
use crate::db::{Database, PersistenceError, TransactionError};
use storage::{DocumentStore, StorageError};

pub(crate) const DOCUMENT_LIMIT: usize = 8 * 1024 * 1024;

pub(crate) struct PrintComponent {
    database: Database,
    documents: DocumentStore,
}

/// Only produced after verifying a dedicated source credential, never an Operator cookie.
#[derive(Clone)]
pub(crate) struct SourceIdentity {
    source_id: String,
    token_hash: [u8; 32],
}

pub(crate) struct PrintSource {
    pub source_id: String,
    pub name: String,
    pub revoked: bool,
}

pub(crate) struct PrintSourceIssued {
    pub source_id: String,
    pub name: String,
    pub revoked: bool,
    pub token: String,
}

#[derive(Clone)]
pub(crate) struct PrintRegistration {
    pub protocol_version: u8,
    pub request_id: String,
    pub account: String,
    /// Display only: at most 1024 UTF-8 bytes, without separators or controls.
    pub original_filename: String,
    pub source_sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenerationFailure {
    CompileTimeout,
    CompileFailed,
    DocumentTooLarge,
    WrapperDeadline,
}

impl GenerationFailure {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CompileTimeout => "compile_timeout",
            Self::CompileFailed => "compile_failed",
            Self::DocumentTooLarge => "document_too_large",
            Self::WrapperDeadline => "wrapper_deadline",
        }
    }

    fn parse(value: &str) -> Result<Self, PrintError> {
        match value {
            "compile_timeout" => Ok(Self::CompileTimeout),
            "compile_failed" => Ok(Self::CompileFailed),
            "document_too_large" => Ok(Self::DocumentTooLarge),
            "wrapper_deadline" => Ok(Self::WrapperDeadline),
            _ => Err(PrintError::Persistence),
        }
    }
}

pub(crate) struct PrintGenerationFailure {
    pub protocol_version: u8,
    pub code: GenerationFailure,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PrintState {
    Preparing,
    GenerationFailed,
    Ready,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PrintDocument {
    pub sha256: String,
    pub bytes: i64,
    pub pages: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PrintAdmission {
    pub ready_sequence: i64,
    pub ahead_tasks: i64,
    pub ahead_pages: i64,
    pub in_progress_tasks: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PrintRequest {
    pub protocol_version: u8,
    pub request_id: String,
    pub account_id: String,
    pub account: String,
    pub team_name: String,
    pub seat_label: Option<String>,
    pub state: PrintState,
    pub document: Option<PrintDocument>,
    pub admission: Option<PrintAdmission>,
    pub generation_failure: Option<GenerationFailure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
pub(crate) enum PrintError {
    #[snafu(display("invalid print request"))]
    Invalid,
    #[snafu(display("print source authentication failed"))]
    Authentication,
    #[snafu(display("print source management requires administrator permission"))]
    Authorization,
    #[snafu(display("print request or source was not found"))]
    NotFound,
    #[snafu(display("print request conflicts with persisted state"))]
    Conflict,
    #[snafu(display("print document exceeds the size limit"))]
    TooLarge,
    #[snafu(display("print persistence failed"))]
    Persistence,
}

impl From<StorageError> for PrintError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::Io { source } => {
                tracing::error!(kind = ?source.kind(), "print document filesystem operation failed");
            }
            other => tracing::error!(error = %other, "print document storage failed"),
        }
        Self::Persistence
    }
}

impl From<PersistenceError> for PrintError {
    fn from(_: PersistenceError) -> Self {
        Self::Persistence
    }
}
impl From<diesel::result::Error> for PrintError {
    fn from(_: diesel::result::Error) -> Self {
        Self::Persistence
    }
}

pub(crate) fn canonical_id(value: &str) -> Result<(), PrintError> {
    let id = Uuid::parse_str(value).map_err(|_| PrintError::Invalid)?;
    if id.get_version_num() != 7
        || id.get_variant() != uuid::Variant::RFC4122
        || id.to_string() != value
    {
        return Err(PrintError::Invalid);
    }
    Ok(())
}

impl PrintComponent {
    #[cfg(test)]
    pub(crate) fn for_test(database: Database, root: PathBuf) -> Result<Self, PrintError> {
        let documents = DocumentStore::open(root)?;
        Ok(Self {
            database,
            documents,
        })
    }

    pub(crate) async fn load(database: Database, root: PathBuf) -> Result<Self, PrintError> {
        let documents = tokio::task::spawn_blocking(move || DocumentStore::open(root))
            .await
            .map_err(|_| PrintError::Persistence)??;
        let component = Self {
            database,
            documents,
        };
        let mut sequence = 0;
        while let Some(document) = component
            .database
            .read(move |tx| db::next_document(tx.connection(), sequence))
            .await
            .map_err(TransactionError::into_error)?
        {
            sequence = document.sequence;
            let store = component.documents.clone();
            tokio::task::spawn_blocking(move || store.verify(&document.hash, document.bytes))
                .await
                .map_err(|_| PrintError::Persistence)??;
        }
        Ok(component)
    }

    pub(crate) async fn create_source(
        &self,
        actor: OperatorIdentity,
        name: String,
    ) -> Result<PrintSourceIssued, PrintError> {
        actor
            .require_admin()
            .map_err(|_| PrintError::Authorization)?;
        if !(1..=128).contains(&name.chars().count()) || name.chars().any(char::is_control) {
            return Err(PrintError::Invalid);
        }
        let source_id = Uuid::now_v7().to_string();
        let mut secret = [0; 32];
        getrandom::fill(&mut secret).map_err(|_| PrintError::Persistence)?;
        let token = format!("nps1.{source_id}.{}", hex::encode(secret));
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.database
            .write(move |tx| {
                db::insert_source(tx, &source_id, &name, &hash)?;
                Ok(PrintSourceIssued {
                    source_id,
                    name,
                    revoked: false,
                    token,
                })
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn list_sources(
        &self,
        actor: OperatorIdentity,
    ) -> Result<Vec<PrintSource>, PrintError> {
        actor
            .require_admin()
            .map_err(|_| PrintError::Authorization)?;
        self.database
            .read(db::list_sources)
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn revoke_source(
        &self,
        actor: OperatorIdentity,
        source_id: String,
    ) -> Result<(), PrintError> {
        actor
            .require_admin()
            .map_err(|_| PrintError::Authorization)?;
        canonical_id(&source_id)?;
        self.database
            .write(move |tx| db::revoke_source(tx, &source_id))
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn authenticate(&self, token: &str) -> Result<SourceIdentity, PrintError> {
        let mut parts = token.split('.');
        let (Some("nps1"), Some(source_id), Some(secret), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(PrintError::Authentication);
        };
        if canonical_id(source_id).is_err() || !lowercase_hash(secret) {
            return Err(PrintError::Authentication);
        }
        let identity = SourceIdentity {
            source_id: source_id.to_owned(),
            token_hash: Sha256::digest(token.as_bytes()).into(),
        };
        self.database
            .read(move |tx| {
                db::authenticate(tx, &identity)?;
                Ok(identity)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn register(
        &self,
        source: SourceIdentity,
        request: PrintRegistration,
    ) -> Result<PrintRequest, PrintError> {
        canonical_id(&request.request_id)?;
        if request.protocol_version != 1
            || !lowercase_hash(&request.source_sha256)
            || request.original_filename.len() > 1024
            || request
                .original_filename
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
        {
            return Err(PrintError::Invalid);
        }
        let result = self
            .database
            .write(move |tx| {
                db::authenticate(tx, &source)?;
                db::register(tx, &source.source_id, &request)
            })
            .await
            .map_err(TransactionError::into_error)?;
        self.verify_ready(result).await
    }

    pub(crate) async fn get(
        &self,
        source: SourceIdentity,
        request_id: String,
    ) -> Result<PrintRequest, PrintError> {
        canonical_id(&request_id)?;
        let result = self
            .database
            .read(move |tx| {
                db::authenticate(tx, &source)?;
                db::request(tx, &source.source_id, &request_id)
            })
            .await
            .map_err(TransactionError::into_error)?;
        self.verify_ready(result).await
    }

    async fn verify_ready(&self, request: PrintRequest) -> Result<PrintRequest, PrintError> {
        if let Some(document) = &request.document {
            let store = self.documents.clone();
            let hash = document.sha256.clone();
            let bytes = document.bytes;
            tokio::task::spawn_blocking(move || store.verify(&hash, bytes))
                .await
                .map_err(|_| PrintError::Persistence)??;
        }
        Ok(request)
    }

    pub(crate) async fn generation_failure(
        &self,
        source: SourceIdentity,
        request_id: String,
        failure: PrintGenerationFailure,
    ) -> Result<PrintRequest, PrintError> {
        canonical_id(&request_id)?;
        if failure.protocol_version != 1 {
            return Err(PrintError::Invalid);
        }
        self.database
            .write(move |tx| {
                db::authenticate(tx, &source)?;
                db::fail(tx, &source.source_id, &request_id, failure.code)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn upload(
        &self,
        source: SourceIdentity,
        request_id: String,
        bytes: Vec<u8>,
    ) -> Result<PrintRequest, PrintError> {
        canonical_id(&request_id)?;
        if bytes.len() > DOCUMENT_LIMIT {
            return Err(PrintError::TooLarge);
        }
        // Reject invalid ownership or state before writing an unreferenced file.
        let current = self.get(source.clone(), request_id.clone()).await?;
        if current.state == PrintState::GenerationFailed {
            return Err(PrintError::Conflict);
        }
        // Parsing and durable file writes happen before taking the SQLite write lock.
        let store = self.documents.clone();
        let length = i64::try_from(bytes.len()).map_err(|_| PrintError::TooLarge)?;
        let (pages, hash) = tokio::task::spawn_blocking(move || {
            let pages = pdf::validate(&bytes)?;
            let hash = hex::encode(Sha256::digest(&bytes));
            if current
                .document
                .as_ref()
                .is_some_and(|doc| doc.sha256 != hash || doc.bytes != length)
            {
                return Err(PrintError::Conflict);
            }
            store.put(&hash, &bytes)?;
            Ok::<_, PrintError>((pages, hash))
        })
        .await
        .map_err(|_| PrintError::Persistence)??;
        self.database
            .write(move |tx| {
                db::authenticate(tx, &source)?;
                db::admit(tx, &source.source_id, &request_id, length, pages, &hash)
            })
            .await
            .map_err(TransactionError::into_error)
    }
}

fn lowercase_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn hash_matches(actual: &[u8], expected: &[u8; 32]) -> bool {
    actual.ct_eq(expected).into()
}
