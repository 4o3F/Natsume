use std::borrow::Cow;

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::component::{
    contest::ContestError, device::DeviceError, import::ImportError, operator::OperatorError,
};

#[derive(Serialize, specta::Type)]
struct ErrorResponse {
    title: Cow<'static, str>,
    status: u16,
    code: &'static str,
}

/// Closed set emitted by the current HTTP API. This type belongs to the HTTP
/// adapter; it is not a domain error or a cross-transport registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::http) enum ApiErrorCode {
    AuthenticationFailed,
    AuthorizationDenied,
    InvalidRequest,
    PayloadTooLarge,
    RateLimited,
    ResourceConflict,
    ResourceNotFound,
    InternalError,
    ServiceUnavailable,
    ImportCandidateInvalid,
    ImportCandidatePending,
    ImportCandidateUnavailable,
    ImportPreviewStale,
    ImportSeatOccupied,
    OperatorLoginNameConflict,
    OperatorLastAdmin,
    OperatorCredentialChanged,
    OperatorLinkUnavailable,
    OperatorCurrentPasswordInvalid,
    OperatorLogoutRequired,
}

impl ApiErrorCode {
    pub(in crate::http) const fn as_str(self) -> &'static str {
        match self {
            Self::AuthenticationFailed => "AUTHENTICATION_FAILED",
            Self::AuthorizationDenied => "AUTHORIZATION_DENIED",
            Self::InvalidRequest => "INVALID_REQUEST",
            Self::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            Self::RateLimited => "RATE_LIMITED",
            Self::ResourceConflict => "RESOURCE_CONFLICT",
            Self::ResourceNotFound => "RESOURCE_NOT_FOUND",
            Self::InternalError => "INTERNAL_ERROR",
            Self::ServiceUnavailable => "SERVICE_UNAVAILABLE",
            Self::ImportCandidateInvalid => "IMPORT_CANDIDATE_INVALID",
            Self::ImportCandidatePending => "IMPORT_CANDIDATE_PENDING",
            Self::ImportCandidateUnavailable => "IMPORT_CANDIDATE_UNAVAILABLE",
            Self::ImportPreviewStale => "IMPORT_PREVIEW_STALE",
            Self::ImportSeatOccupied => "IMPORT_SEAT_OCCUPIED",
            Self::OperatorLoginNameConflict => "OPERATOR_LOGIN_NAME_CONFLICT",
            Self::OperatorLastAdmin => "OPERATOR_LAST_ADMIN",
            Self::OperatorCredentialChanged => "OPERATOR_CREDENTIAL_CHANGED",
            Self::OperatorLinkUnavailable => "OPERATOR_LINK_UNAVAILABLE",
            Self::OperatorCurrentPasswordInvalid => "OPERATOR_CURRENT_PASSWORD_INVALID",
            Self::OperatorLogoutRequired => "OPERATOR_LOGOUT_REQUIRED",
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::http) struct ApiErrorMetadata {
    /// Closed public API code selected by the HTTP adapter.
    pub(in crate::http) code: ApiErrorCode,
    /// Compile-time failure discriminant; excludes titles and source errors.
    pub(in crate::http) cause: &'static str,
}

pub(super) struct ApiError {
    status: StatusCode,
    title: Cow<'static, str>,
    code: ApiErrorCode,
    /// Compile-time constant discriminant of the internal failure mode. It is
    /// logged on the current request Span, never serialized or sent as a
    /// header.
    cause: &'static str,
    reported: bool,
}

impl ApiError {
    pub(super) fn from_photo_enrollment(error: crate::component::photo::EnrollmentError) -> Self {
        use crate::component::photo::EnrollmentError;
        match error {
            EnrollmentError::Authentication => {
                Self::authentication_failed("photo_credential_rejected")
            }
            EnrollmentError::NotFound => Self::not_found("photo_source_not_found"),
            EnrollmentError::Conflict => Self::conflict("photo_source_conflict"),
            EnrollmentError::InvalidId => Self::invalid_request("photo_identifier"),
            EnrollmentError::InvalidName => Self::invalid_request("photo_source_name"),
            EnrollmentError::InvalidPairing => Self::invalid_request("photo_pairing_request"),
            EnrollmentError::InvalidKey => Self::invalid_request("photo_signing_key"),
            EnrollmentError::RateLimited => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "Too Many Requests",
                ApiErrorCode::RateLimited,
                "photo_pairing_limit",
            ),
            EnrollmentError::Persistence { ref source } if source.is_transient() => {
                Self::unavailable("photo_database_busy").report_from(error)
            }
            EnrollmentError::Persistence { .. } => {
                Self::internal_error_from(error, "photo_enrollment_persistence")
            }
            EnrollmentError::Entropy { .. } => {
                Self::internal_error_from(error, "photo_enrollment_entropy")
            }
        }
    }

    pub(super) fn from_photo_initialization(
        error: crate::component::photo::InitializationError,
    ) -> Self {
        use crate::component::photo::InitializationError;
        match error {
            InitializationError::Enrollment { source } => Self::from_photo_enrollment(source),
            InitializationError::Unconfirmed => {
                Self::authorization_denied("photo_enrollment_unconfirmed")
            }
            InitializationError::InvalidRequest => {
                Self::invalid_request("photo_initialization_request")
            }
            InitializationError::NotFound => Self::not_found("photo_initialization_resource"),
            InitializationError::Conflict => Self::conflict("photo_initialization_conflict"),
            InitializationError::EmptyRoster => Self::conflict("photo_roster_empty"),
            InitializationError::Busy => Self::unavailable("photo_initialization_busy"),
            InitializationError::Limit => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Payload Too Large",
                ApiErrorCode::PayloadTooLarge,
                "photo_initialization_limit",
            ),
            InitializationError::Persistence { ref source } if source.is_transient() => {
                Self::unavailable("photo_database_busy").report_from(error)
            }
            InitializationError::Persistence { .. } => {
                Self::internal_error_from(error, "photo_initialization_persistence")
            }
            InitializationError::Corrupt => {
                Self::internal_error_from(error, "photo_initialization_corrupt")
            }
            InitializationError::Roster { .. } => {
                Self::internal_error_from(error, "photo_initialization_roster")
            }
            InitializationError::Logo { .. } => {
                Self::internal_error_from(error, "photo_initialization_logo")
            }
            InitializationError::Io { .. } => {
                Self::internal_error_from(error, "photo_initialization_io")
            }
            InitializationError::Json { .. } => {
                Self::internal_error_from(error, "photo_initialization_json")
            }
        }
    }

    pub(super) fn internal_error_from<E: std::error::Error>(error: E, cause: &'static str) -> Self {
        Self::internal_error(cause).report_from(error)
    }

    fn report_from<E: std::error::Error>(mut self, error: E) -> Self {
        if self.status.is_server_error() && self.status != StatusCode::SERVICE_UNAVAILABLE {
            tracing::error!(code = self.code.as_str(), cause = self.cause, error = %snafu::Report::from_error(error), "HTTP request failed");
        } else {
            tracing::warn!(code = self.code.as_str(), cause = self.cause, error = %snafu::Report::from_error(error), "HTTP request rejected");
        }
        self.reported = true;
        self
    }

    pub(super) fn unavailable(cause: &'static str) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service Unavailable",
            ApiErrorCode::ServiceUnavailable,
            cause,
        )
    }

    pub(super) fn authentication_failed(cause: &'static str) -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            ApiErrorCode::AuthenticationFailed,
            cause,
        )
    }

    pub(super) fn authorization_denied(cause: &'static str) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "Forbidden",
            ApiErrorCode::AuthorizationDenied,
            cause,
        )
    }

    pub(super) fn invalid_request(cause: &'static str) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "Bad Request",
            ApiErrorCode::InvalidRequest,
            cause,
        )
    }

    pub(super) fn not_found(cause: &'static str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "Not Found",
            ApiErrorCode::ResourceNotFound,
            cause,
        )
    }

    pub(super) fn conflict(cause: &'static str) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "Conflict",
            ApiErrorCode::ResourceConflict,
            cause,
        )
    }

    pub(super) fn internal_error(cause: &'static str) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            ApiErrorCode::InternalError,
            cause,
        )
    }

    /// Each arm names its own static discriminant, so the exhaustive match keeps
    /// every typed failure mode distinguishable in logs without widening the
    /// published `code`.
    pub(super) fn from_operator(error: OperatorError) -> Self {
        match error {
            OperatorError::SignInBusy => Self::unavailable("operator_sign_in_busy"),
            OperatorError::CredentialsTooLong => {
                Self::invalid_request("operator_credentials_too_long")
            }
            OperatorError::AuthenticationFailed => {
                Self::authentication_failed("operator_authentication_failed")
            }
            OperatorError::SessionAuthenticationFailed => {
                Self::authentication_failed("operator_session_authentication_failed")
            }
            OperatorError::SessionCleanup { .. } => {
                Self::authentication_failed("operator_store_transaction_failed").report_from(error)
            }
            OperatorError::InvalidSessionCredential => {
                Self::authentication_failed("operator_invalid_session_credential")
            }
            OperatorError::AuthorizationDenied => {
                Self::authorization_denied("operator_authorization_denied")
            }
            OperatorError::PersistenceFailed { .. } => {
                Self::internal_error_from(error, "operator_persistence_failed")
            }
            OperatorError::PasswordTaskFailed { .. } => {
                Self::internal_error_from(error, "operator_password_task_failed")
            }
            OperatorError::PasswordVerificationFailed
            | OperatorError::PasswordVerification { .. } => {
                Self::internal_error_from(error, "operator_password_verification_failed")
            }
            OperatorError::InvalidPersistedIdentity { .. } => {
                Self::internal_error_from(error, "operator_invalid_persisted_identity")
            }
            OperatorError::InvalidPersistedRole => {
                Self::internal_error_from(error, "operator_invalid_persisted_role")
            }
            OperatorError::EntropyUnavailable { .. } => {
                Self::internal_error_from(error, "operator_entropy_unavailable")
            }
            OperatorError::InvalidHashingParameters { .. } => {
                Self::internal_error_from(error, "operator_invalid_hashing_parameters")
            }
            OperatorError::SaltEncodingFailed { .. } => {
                Self::internal_error_from(error, "operator_salt_encoding_failed")
            }
            OperatorError::PasswordHashingFailed { .. } => {
                Self::internal_error_from(error, "operator_password_hashing_failed")
            }
            OperatorError::EmptyLoginName => Self::invalid_request("operator_empty_login_name"),
            OperatorError::InvalidNewLoginName => {
                Self::invalid_request("operator_invalid_new_login_name")
            }
            OperatorError::InvalidNewPassword => {
                Self::invalid_request("operator_invalid_new_password")
            }
            OperatorError::PasswordMismatch => Self::invalid_request("operator_password_mismatch"),
            OperatorError::OperatorNotFound => Self::not_found("operator_not_found"),
            OperatorError::LoginNameConflict => Self::new(
                StatusCode::CONFLICT,
                "Conflict",
                ApiErrorCode::OperatorLoginNameConflict,
                "operator_login_name_conflict",
            ),
            OperatorError::LastAdmin => Self::new(
                StatusCode::CONFLICT,
                "Conflict",
                ApiErrorCode::OperatorLastAdmin,
                "operator_last_admin",
            ),
            OperatorError::LinkUnavailable => Self::new(
                StatusCode::GONE,
                "Gone",
                ApiErrorCode::OperatorLinkUnavailable,
                "operator_link_unavailable",
            ),
            OperatorError::CredentialChanged => Self::new(
                StatusCode::CONFLICT,
                "Conflict",
                ApiErrorCode::OperatorCredentialChanged,
                "operator_credential_changed",
            ),
        }
    }

    pub(super) fn from_operator_password_change(error: OperatorError) -> Self {
        match error {
            OperatorError::AuthenticationFailed => Self::new(
                StatusCode::BAD_REQUEST,
                "Bad Request",
                ApiErrorCode::OperatorCurrentPasswordInvalid,
                "operator_current_password_invalid",
            ),
            other => Self::from_operator(other),
        }
    }

    pub(super) fn operator_logout_required() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "Conflict",
            ApiErrorCode::OperatorLogoutRequired,
            "operator_logout_required",
        )
    }

    pub(super) fn from_contest(error: ContestError) -> Self {
        match error {
            ContestError::PersistenceFailed { .. } => {
                Self::internal_error_from(error, "contest_persistence_failed")
            }
        }
    }

    pub(super) fn from_device(error: impl std::borrow::Borrow<DeviceError>) -> Self {
        let error = error.borrow();
        match error {
            DeviceError::ServerStopping => Self::unavailable("server_stopping"),
            DeviceError::DeviceNotFound => Self::not_found("device_not_found"),
            DeviceError::InvalidPersistedFacts { .. } => {
                Self::internal_error_from(error, "device_invalid_persisted_facts")
            }
            DeviceError::PersistenceFailed { .. } => {
                Self::internal_error_from(error, "device_persistence_failed")
            }
        }
    }

    pub(super) fn from_import(error: ImportError) -> Self {
        match error {
            ImportError::Busy => Self::unavailable("import_busy"),
            ImportError::InvalidWorkbook { source: diagnostic } => {
                let mut error = Self::new(
                    StatusCode::BAD_REQUEST,
                    "Bad Request",
                    ApiErrorCode::ImportCandidateInvalid,
                    "import_workbook_invalid",
                );
                error.title = diagnostic.to_string().into();
                error
            }
            ImportError::CandidateWorker { .. } | ImportError::CandidateEncoding { .. } => {
                Self::new(
                    StatusCode::BAD_REQUEST,
                    "Bad Request",
                    ApiErrorCode::ImportCandidateInvalid,
                    "import_candidate_invalid",
                )
                .report_from(error)
            }
            ImportError::CandidateInvalid => Self::new(
                StatusCode::BAD_REQUEST,
                "Bad Request",
                ApiErrorCode::ImportCandidateInvalid,
                "import_candidate_invalid",
            ),
            ImportError::CandidatePending => Self::new(
                StatusCode::CONFLICT,
                "Conflict",
                ApiErrorCode::ImportCandidatePending,
                "import_candidate_pending",
            ),
            ImportError::CandidateUnavailable => Self::new(
                StatusCode::NOT_FOUND,
                "Not Found",
                ApiErrorCode::ImportCandidateUnavailable,
                "import_candidate_unavailable",
            ),
            ImportError::PreviewStale => Self::new(
                StatusCode::CONFLICT,
                "Conflict",
                ApiErrorCode::ImportPreviewStale,
                "import_preview_stale",
            ),
            ImportError::SeatOccupied => Self::new(
                StatusCode::CONFLICT,
                "Conflict",
                ApiErrorCode::ImportSeatOccupied,
                "import_seat_occupied",
            ),
            ImportError::EntropyUnavailable { .. } => {
                Self::internal_error_from(error, "import_entropy_unavailable")
            }
            ImportError::VaultFailure { .. } => {
                Self::internal_error_from(error, "import_vault_failure")
            }
            ImportError::PersistenceFailure { .. } => {
                Self::internal_error_from(error, "import_persistence_failure")
            }
        }
    }

    pub(super) fn from_export(error: &crate::component::contest::ExportError) -> Self {
        use crate::component::contest::ExportError;
        let mut response = match error {
            ExportError::Busy => Self::unavailable("roster_export_busy"),
            ExportError::IncompleteRoster => Self::conflict("roster_export_incomplete"),
            ExportError::LogoUnavailable => Self::not_found("organization_logo_unavailable"),
            ExportError::Persistence { .. } => {
                Self::internal_error_from(error, "roster_export_persistence")
            }
            ExportError::Vault { .. } | ExportError::CredentialText { .. } => {
                Self::internal_error_from(error, "roster_export_vault")
            }
            ExportError::LogoDirectory { .. } => {
                Self::internal_error_from(error, "organization_logo_directory")
            }
            ExportError::Logo { .. } => {
                Self::internal_error_from(error, "organization_logo_invalid")
            }
            ExportError::Archive
            | ExportError::ArchiveZip { .. }
            | ExportError::ArchiveIo { .. }
            | ExportError::ArchiveJson { .. } => {
                Self::internal_error_from(error, "roster_export_archive")
            }
            ExportError::Worker { .. } | ExportError::WorkerUnavailable { .. } => {
                Self::internal_error_from(error, "roster_export_worker")
            }
        };
        response.title = error.to_string().into();
        response
    }

    pub(super) fn from_print(error: crate::component::print::PrintError) -> Self {
        use crate::component::print::PrintError;
        match error {
            PrintError::Invalid => Self::invalid_request("print_invalid"),
            PrintError::Authentication => Self::authentication_failed("print_authentication"),
            PrintError::NotFound => Self::not_found("print_not_found"),
            PrintError::Conflict => Self::conflict("print_conflict"),
            PrintError::TooLarge => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Payload Too Large",
                ApiErrorCode::PayloadTooLarge,
                "print_document_too_large",
            ),
            PrintError::Persistence { .. }
            | PrintError::Storage { .. }
            | PrintError::Executor { .. }
            | PrintError::Entropy { .. }
            | PrintError::Expiry { .. }
            | PrintError::StatePoisoned { .. } => {
                Self::internal_error_from(error, "print_persistence")
            }
            PrintError::RateLimited => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "Too Many Requests",
                ApiErrorCode::RateLimited,
                "print_pairing_rate_limit",
            ),
        }
    }

    fn new(
        status: StatusCode,
        title: &'static str,
        code: ApiErrorCode,
        cause: &'static str,
    ) -> Self {
        Self {
            status,
            title: Cow::Borrowed(title),
            code,
            cause,
            reported: false,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let error_response = ErrorResponse {
            title: self.title,
            status: self.status.as_u16(),
            code: self.code.as_str(),
        };
        if !self.reported {
            if self.status.is_server_error() && self.status != StatusCode::SERVICE_UNAVAILABLE {
                tracing::error!(
                    code = self.code.as_str(),
                    cause = self.cause,
                    "HTTP request failed"
                );
            } else {
                tracing::warn!(
                    code = self.code.as_str(),
                    cause = self.cause,
                    "HTTP request rejected"
                );
            }
        }
        let mut response = (self.status, Json(error_response)).into_response();
        response.extensions_mut().insert(ApiErrorMetadata {
            code: self.code,
            cause: self.cause,
        });
        if self.status == StatusCode::SERVICE_UNAVAILABLE {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        if self.status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
        }
        response
    }
}

#[cfg(test)]
mod tests;
