//! Durable `DOMjudge` admission, separate from Device Control and physical printing.
mod db;
mod pdf;
#[cfg(test)]
pub(crate) mod tests;

use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use uuid::Uuid;

use super::operator::OperatorIdentity;
use crate::db::{Database, PersistenceError, TransactionError};

pub(crate) const DOCUMENT_LIMIT: usize = 8 * 1024 * 1024;

pub(crate) struct PrintComponent {
    database: Database,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrintError {
    Invalid,
    Authentication,
    Authorization,
    NotFound,
    Conflict,
    TooLarge,
    Persistence,
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
    pub(crate) const fn new(database: Database) -> Self {
        Self { database }
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
        self.database
            .write(move |tx| {
                db::authenticate(tx, &source)?;
                db::register(tx, &source.source_id, &request)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn get(
        &self,
        source: SourceIdentity,
        request_id: String,
    ) -> Result<PrintRequest, PrintError> {
        canonical_id(&request_id)?;
        self.database
            .read(move |tx| {
                db::authenticate(tx, &source)?;
                db::request(tx, &source.source_id, &request_id)
            })
            .await
            .map_err(TransactionError::into_error)
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
        // Parse off the async executor, before taking the SQLite write lock.
        let (bytes, pages, hash) = tokio::task::spawn_blocking(move || {
            let pages = pdf::validate(&bytes)?;
            let hash = hex::encode(Sha256::digest(&bytes));
            Ok::<_, PrintError>((bytes, pages, hash))
        })
        .await
        .map_err(|_| PrintError::Persistence)??;
        self.database
            .write(move |tx| {
                db::authenticate(tx, &source)?;
                db::admit(tx, &source.source_id, &request_id, &bytes, pages, &hash)
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
