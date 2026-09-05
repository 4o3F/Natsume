use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::component::{
    contest::ContestError,
    device::DeviceError,
    import::{CsvImportErrorCategory, ImportError},
    operator::OperatorError,
};

#[derive(Serialize)]
struct ErrorResponse {
    title: &'static str,
    status: u16,
    code: &'static str,
}

/// Closed set emitted by the current HTTP API. This type belongs to the HTTP
/// adapter; it is not a domain error or a cross-transport registry.
#[derive(Clone, Copy)]
enum ApiErrorCode {
    AuthenticationFailed,
    AuthorizationDenied,
    InvalidRequest,
    ResourceConflict,
    ResourceNotFound,
    InternalError,
    ImportCandidateInvalid,
    ImportCandidatePending,
    ImportCandidateUnavailable,
    ImportPreviewStale,
    ImportSeatOccupied,
}

impl ApiErrorCode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::AuthenticationFailed => "AUTHENTICATION_FAILED",
            Self::AuthorizationDenied => "AUTHORIZATION_DENIED",
            Self::InvalidRequest => "INVALID_REQUEST",
            Self::ResourceConflict => "RESOURCE_CONFLICT",
            Self::ResourceNotFound => "RESOURCE_NOT_FOUND",
            Self::InternalError => "INTERNAL_ERROR",
            Self::ImportCandidateInvalid => "IMPORT_CANDIDATE_INVALID",
            Self::ImportCandidatePending => "IMPORT_CANDIDATE_PENDING",
            Self::ImportCandidateUnavailable => "IMPORT_CANDIDATE_UNAVAILABLE",
            Self::ImportPreviewStale => "IMPORT_PREVIEW_STALE",
            Self::ImportSeatOccupied => "IMPORT_SEAT_OCCUPIED",
        }
    }
}

pub(super) struct ApiError {
    status: StatusCode,
    title: &'static str,
    code: ApiErrorCode,
    /// Compile-time constant discriminant of the internal failure mode. It is
    /// logged on the current request Span, never serialized or sent as a
    /// header.
    cause: &'static str,
}

impl ApiError {
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
            OperatorError::AuthenticationFailed => {
                Self::authentication_failed("operator_authentication_failed")
            }
            OperatorError::SessionAuthenticationFailed => {
                Self::authentication_failed("operator_session_authentication_failed")
            }
            OperatorError::InvalidSessionCredential => {
                Self::authentication_failed("operator_invalid_session_credential")
            }
            OperatorError::AuthorizationDenied => {
                Self::authorization_denied("operator_authorization_denied")
            }
            OperatorError::PersistenceFailed => Self::internal_error("operator_persistence_failed"),
            OperatorError::PasswordTaskFailed => {
                Self::internal_error("operator_password_task_failed")
            }
            OperatorError::PasswordVerificationFailed => {
                Self::internal_error("operator_password_verification_failed")
            }
            OperatorError::InvalidPersistedIdentity => {
                Self::internal_error("operator_invalid_persisted_identity")
            }
            OperatorError::InvalidPersistedRole => {
                Self::internal_error("operator_invalid_persisted_role")
            }
            OperatorError::EntropyUnavailable => {
                Self::internal_error("operator_entropy_unavailable")
            }
            OperatorError::InvalidHashingParameters => {
                Self::internal_error("operator_invalid_hashing_parameters")
            }
            OperatorError::SaltEncodingFailed => {
                Self::internal_error("operator_salt_encoding_failed")
            }
            OperatorError::PasswordHashingFailed => {
                Self::internal_error("operator_password_hashing_failed")
            }
            // These two are bootstrap-only input failures. If either ever
            // crosses an HTTP boundary, it is an internal wiring fault rather
            // than a newly published request-validation semantic.
            OperatorError::EmptyLoginName => Self::internal_error("operator_empty_login_name"),
            OperatorError::PasswordMismatch => Self::internal_error("operator_password_mismatch"),
        }
    }

    pub(super) fn from_contest(error: ContestError) -> Self {
        match error {
            ContestError::PersistenceFailed => Self::internal_error("contest_persistence_failed"),
        }
    }

    pub(super) fn from_device(error: DeviceError) -> Self {
        match error {
            DeviceError::DeviceNotFound => Self::not_found("device_not_found"),
            DeviceError::InvalidPersistedFacts => {
                Self::internal_error("device_invalid_persisted_facts")
            }
            DeviceError::PersistenceFailed => Self::internal_error("device_persistence_failed"),
        }
    }

    pub(super) fn from_import(error: ImportError) -> Self {
        match error {
            ImportError::InvalidCsv(category) => {
                let cause = match category {
                    CsvImportErrorCategory::InvalidUtf8 => "import_csv_invalid_utf8",
                    CsvImportErrorCategory::InvalidHeader => "import_csv_invalid_header",
                    CsvImportErrorCategory::WrongColumnCount => "import_csv_wrong_column_count",
                    CsvImportErrorCategory::EmptyField => "import_csv_empty_field",
                    CsvImportErrorCategory::FieldTooLong => "import_csv_field_too_long",
                    CsvImportErrorCategory::ControlCharacter => "import_csv_control_character",
                    CsvImportErrorCategory::DuplicateSeatCode => "import_csv_duplicate_seat_code",
                    CsvImportErrorCategory::DuplicateAccountUsername => {
                        "import_csv_duplicate_account_username"
                    }
                    CsvImportErrorCategory::TooManyRows => "import_csv_too_many_rows",
                    CsvImportErrorCategory::ZeroDataRows => "import_csv_zero_data_rows",
                };
                Self::new(
                    StatusCode::BAD_REQUEST,
                    "Bad Request",
                    ApiErrorCode::ImportCandidateInvalid,
                    cause,
                )
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
            ImportError::EntropyUnavailable => Self::internal_error("import_entropy_unavailable"),
            ImportError::VaultFailure => Self::internal_error("import_vault_failure"),
            ImportError::PersistenceFailure => Self::internal_error("import_persistence_failure"),
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
            title,
            code,
            cause,
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
        if self.status.is_server_error() {
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
        (self.status, Json(error_response)).into_response()
    }
}

#[cfg(test)]
mod tests;
