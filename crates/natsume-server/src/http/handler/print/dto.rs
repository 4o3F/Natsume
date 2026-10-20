use serde::{Deserialize, Serialize};

use crate::component::print as domain;

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintSource {
    source_id: String,
    name: String,
    revoked: bool,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintSourceIssued {
    source_id: String,
    name: String,
    revoked: bool,
    token: String,
}

#[derive(Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintRegistration {
    protocol_version: u8,
    request_id: String,
    account: String,
    /// Display only: at most 1024 UTF-8 bytes, without separators or controls.
    original_filename: String,
    source_sha256: String,
}

#[derive(Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GenerationFailure {
    CompileTimeout,
    CompileFailed,
    DocumentTooLarge,
    WrapperDeadline,
}

#[derive(Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintGenerationFailure {
    protocol_version: u8,
    code: GenerationFailure,
}

#[derive(Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PrintState {
    Preparing,
    GenerationFailed,
    Ready,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintDocument {
    sha256: String,
    #[specta(type = specta_typescript::Number)]
    bytes: i64,
    #[specta(type = specta_typescript::Number)]
    pages: i64,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintAdmission {
    #[specta(type = specta_typescript::Number)]
    ready_sequence: i64,
    #[specta(type = specta_typescript::Number)]
    ahead_tasks: i64,
    #[specta(type = specta_typescript::Number)]
    ahead_pages: i64,
    #[specta(type = specta_typescript::Number)]
    in_progress_tasks: i64,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintRequest {
    protocol_version: u8,
    request_id: String,
    account_id: String,
    account: String,
    team_name: String,
    seat_label: Option<String>,
    state: PrintState,
    document: Option<PrintDocument>,
    admission: Option<PrintAdmission>,
    generation_failure: Option<GenerationFailure>,
}

impl From<domain::PrintSource> for PrintSource {
    fn from(value: domain::PrintSource) -> Self {
        Self {
            source_id: value.source_id,
            name: value.name,
            revoked: value.revoked,
        }
    }
}

impl From<domain::PrintSourceIssued> for PrintSourceIssued {
    fn from(value: domain::PrintSourceIssued) -> Self {
        Self {
            source_id: value.source_id,
            name: value.name,
            revoked: value.revoked,
            token: value.token,
        }
    }
}

impl From<PrintRegistration> for domain::PrintRegistration {
    fn from(value: PrintRegistration) -> Self {
        Self {
            protocol_version: value.protocol_version,
            request_id: value.request_id,
            account: value.account,
            original_filename: value.original_filename,
            source_sha256: value.source_sha256,
        }
    }
}

impl From<GenerationFailure> for domain::GenerationFailure {
    fn from(value: GenerationFailure) -> Self {
        match value {
            GenerationFailure::CompileTimeout => Self::CompileTimeout,
            GenerationFailure::CompileFailed => Self::CompileFailed,
            GenerationFailure::DocumentTooLarge => Self::DocumentTooLarge,
            GenerationFailure::WrapperDeadline => Self::WrapperDeadline,
        }
    }
}

impl From<domain::GenerationFailure> for GenerationFailure {
    fn from(value: domain::GenerationFailure) -> Self {
        match value {
            domain::GenerationFailure::CompileTimeout => Self::CompileTimeout,
            domain::GenerationFailure::CompileFailed => Self::CompileFailed,
            domain::GenerationFailure::DocumentTooLarge => Self::DocumentTooLarge,
            domain::GenerationFailure::WrapperDeadline => Self::WrapperDeadline,
        }
    }
}

impl From<PrintGenerationFailure> for domain::PrintGenerationFailure {
    fn from(value: PrintGenerationFailure) -> Self {
        Self {
            protocol_version: value.protocol_version,
            code: value.code.into(),
        }
    }
}

impl From<domain::PrintState> for PrintState {
    fn from(value: domain::PrintState) -> Self {
        match value {
            domain::PrintState::Preparing => Self::Preparing,
            domain::PrintState::GenerationFailed => Self::GenerationFailed,
            domain::PrintState::Ready => Self::Ready,
        }
    }
}

impl From<domain::PrintDocument> for PrintDocument {
    fn from(value: domain::PrintDocument) -> Self {
        Self {
            sha256: value.sha256,
            bytes: value.bytes,
            pages: value.pages,
        }
    }
}

impl From<domain::PrintAdmission> for PrintAdmission {
    fn from(value: domain::PrintAdmission) -> Self {
        Self {
            ready_sequence: value.ready_sequence,
            ahead_tasks: value.ahead_tasks,
            ahead_pages: value.ahead_pages,
            in_progress_tasks: value.in_progress_tasks,
        }
    }
}

impl From<domain::PrintRequest> for PrintRequest {
    fn from(value: domain::PrintRequest) -> Self {
        Self {
            protocol_version: value.protocol_version,
            request_id: value.request_id,
            account_id: value.account_id,
            account: value.account,
            team_name: value.team_name,
            seat_label: value.seat_label,
            state: value.state.into(),
            document: value.document.map(Into::into),
            admission: value.admission.map(Into::into),
            generation_failure: value.generation_failure.map(Into::into),
        }
    }
}
