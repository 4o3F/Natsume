use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::{
    audit::CorrelationId,
    component::{
        contest::ContestError,
        import::{CsvImportErrorCategory, ImportError},
        lifecycle::DeviceError,
        operator::OperatorError,
        provisioning::ProvisioningError,
    },
};

#[derive(Serialize)]
struct ErrorResponse<'a> {
    title: &'static str,
    status: u16,
    code: &'static str,
    correlation_id: &'a str,
}

/// Closed set emitted by the current HTTP API. This type belongs to the HTTP
/// adapter; it is not a domain error or a cross-transport registry.
#[derive(Clone, Copy)]
enum ApiErrorCode {
    AuthenticationFailed,
    AuthorizationDenied,
    InvalidRequest,
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
    /// Compile-time constant discriminant of the internal failure mode, logged
    /// beside the correlation ID so a published `code` stays attributable. It
    /// is never serialized into the body and never sent as a header.
    cause: &'static str,
    correlation_id: CorrelationId,
}

impl ApiError {
    pub(super) fn authentication_failed(
        cause: &'static str,
        correlation_id: CorrelationId,
    ) -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            ApiErrorCode::AuthenticationFailed,
            cause,
            correlation_id,
        )
    }

    pub(super) fn authorization_denied(cause: &'static str, correlation_id: CorrelationId) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "Forbidden",
            ApiErrorCode::AuthorizationDenied,
            cause,
            correlation_id,
        )
    }

    pub(super) fn invalid_request(cause: &'static str, correlation_id: CorrelationId) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "Bad Request",
            ApiErrorCode::InvalidRequest,
            cause,
            correlation_id,
        )
    }

    pub(super) fn not_found(cause: &'static str, correlation_id: CorrelationId) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "Not Found",
            ApiErrorCode::ResourceNotFound,
            cause,
            correlation_id,
        )
    }

    pub(super) fn internal_error(cause: &'static str, correlation_id: CorrelationId) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            ApiErrorCode::InternalError,
            cause,
            correlation_id,
        )
    }

    /// Each arm names its own static discriminant, so the exhaustive match keeps
    /// every typed failure mode distinguishable in logs without widening the
    /// published `code`.
    pub(super) fn from_operator(error: OperatorError, correlation_id: CorrelationId) -> Self {
        match error {
            OperatorError::AuthenticationFailed => {
                Self::authentication_failed("operator_authentication_failed", correlation_id)
            }
            OperatorError::SessionAuthenticationFailed => Self::authentication_failed(
                "operator_session_authentication_failed",
                correlation_id,
            ),
            OperatorError::InvalidSessionCredential => {
                Self::authentication_failed("operator_invalid_session_credential", correlation_id)
            }
            OperatorError::AuthorizationDenied => {
                Self::authorization_denied("operator_authorization_denied", correlation_id)
            }
            OperatorError::PersistenceFailed => {
                Self::internal_error("operator_persistence_failed", correlation_id)
            }
            OperatorError::PasswordTaskFailed => {
                Self::internal_error("operator_password_task_failed", correlation_id)
            }
            OperatorError::PasswordVerificationFailed => {
                Self::internal_error("operator_password_verification_failed", correlation_id)
            }
            OperatorError::InvalidPersistedIdentity => {
                Self::internal_error("operator_invalid_persisted_identity", correlation_id)
            }
            OperatorError::InvalidPersistedRole => {
                Self::internal_error("operator_invalid_persisted_role", correlation_id)
            }
            OperatorError::EntropyUnavailable => {
                Self::internal_error("operator_entropy_unavailable", correlation_id)
            }
            OperatorError::InvalidHashingParameters => {
                Self::internal_error("operator_invalid_hashing_parameters", correlation_id)
            }
            OperatorError::SaltEncodingFailed => {
                Self::internal_error("operator_salt_encoding_failed", correlation_id)
            }
            OperatorError::PasswordHashingFailed => {
                Self::internal_error("operator_password_hashing_failed", correlation_id)
            }
            // These two are bootstrap-only input failures. If either ever
            // crosses an HTTP boundary, it is an internal wiring fault rather
            // than a newly published request-validation semantic.
            OperatorError::EmptyLoginName => {
                Self::internal_error("operator_empty_login_name", correlation_id)
            }
            OperatorError::PasswordMismatch => {
                Self::internal_error("operator_password_mismatch", correlation_id)
            }
        }
    }

    pub(super) fn from_contest(error: ContestError, correlation_id: CorrelationId) -> Self {
        match error {
            ContestError::PersistenceFailed => {
                Self::internal_error("contest_persistence_failed", correlation_id)
            }
        }
    }

    #[allow(dead_code)]
    pub(super) fn from_device(error: DeviceError, correlation_id: CorrelationId) -> Self {
        match error {
            DeviceError::InvalidDeviceId => {
                Self::invalid_request("contest_invalid_device_id", correlation_id)
            }
            DeviceError::DeviceNotFound => {
                Self::not_found("contest_device_not_found", correlation_id)
            }
            DeviceError::InvalidPersistedFacts => {
                Self::internal_error("contest_invalid_persisted_facts", correlation_id)
            }
            DeviceError::PersistenceFailed => {
                Self::internal_error("contest_persistence_failed", correlation_id)
            }
        }
    }

    pub(super) fn from_import(error: ImportError, correlation_id: CorrelationId) -> Self {
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
                Self::import_error(
                    StatusCode::BAD_REQUEST,
                    "Bad Request",
                    ApiErrorCode::ImportCandidateInvalid,
                    cause,
                    correlation_id,
                )
            }
            ImportError::CandidateInvalid => Self::import_error(
                StatusCode::BAD_REQUEST,
                "Bad Request",
                ApiErrorCode::ImportCandidateInvalid,
                "import_candidate_invalid",
                correlation_id,
            ),
            ImportError::CandidatePending => Self::import_error(
                StatusCode::CONFLICT,
                "Conflict",
                ApiErrorCode::ImportCandidatePending,
                "import_candidate_pending",
                correlation_id,
            ),
            ImportError::CandidateUnavailable => Self::import_error(
                StatusCode::NOT_FOUND,
                "Not Found",
                ApiErrorCode::ImportCandidateUnavailable,
                "import_candidate_unavailable",
                correlation_id,
            ),
            ImportError::PreviewStale => Self::import_error(
                StatusCode::CONFLICT,
                "Conflict",
                ApiErrorCode::ImportPreviewStale,
                "import_preview_stale",
                correlation_id,
            ),
            ImportError::SeatOccupied => Self::import_error(
                StatusCode::CONFLICT,
                "Conflict",
                ApiErrorCode::ImportSeatOccupied,
                "import_seat_occupied",
                correlation_id,
            ),
            ImportError::EntropyUnavailable => {
                Self::internal_error("import_entropy_unavailable", correlation_id)
            }
            ImportError::VaultFailure => {
                Self::internal_error("import_vault_failure", correlation_id)
            }
            ImportError::PersistenceFailure => {
                Self::internal_error("import_persistence_failure", correlation_id)
            }
        }
    }

    pub(super) fn from_provisioning(
        error: ProvisioningError,
        correlation_id: CorrelationId,
    ) -> Self {
        match error {
            ProvisioningError::PersistenceFailed => {
                Self::internal_error("provisioning_persistence_failed", correlation_id)
            }
        }
    }

    fn import_error(
        status: StatusCode,
        title: &'static str,
        code: ApiErrorCode,
        cause: &'static str,
        correlation_id: CorrelationId,
    ) -> Self {
        Self {
            status,
            title,
            code,
            cause,
            correlation_id,
        }
    }

    fn new(
        status: StatusCode,
        title: &'static str,
        code: ApiErrorCode,
        cause: &'static str,
        correlation_id: CorrelationId,
    ) -> Self {
        Self {
            status,
            title,
            code,
            cause,
            correlation_id,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let correlation_id = self.correlation_id.as_text();
        let error_response = ErrorResponse {
            title: self.title,
            status: self.status.as_u16(),
            code: self.code.as_str(),
            correlation_id: &correlation_id,
        };
        let body = serde_json::to_vec(&error_response).unwrap_or_else(|_| {
            tracing::error!(
                code = ApiErrorCode::InternalError.as_str(),
                cause = "http_error_response_serialization_failed",
                correlation_id = %correlation_id,
                "HTTP error response serialization invariant failed"
            );
            panic!("HTTP error response serialization invariant failed");
        });

        if self.status.is_server_error() {
            tracing::error!(
                code = self.code.as_str(),
                cause = self.cause,
                correlation_id = %correlation_id,
                "HTTP request failed"
            );
        } else {
            tracing::warn!(
                code = self.code.as_str(),
                cause = self.cause,
                correlation_id = %correlation_id,
                "HTTP request rejected"
            );
        }
        (
            self.status,
            [(header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests;
