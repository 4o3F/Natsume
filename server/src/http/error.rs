use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use natsume_error_code::{
    ErrorCode, common::CommonErrorCode, control::ControlErrorCode,
    enrollment::EnrollmentErrorCode as PublicEnrollmentErrorCode,
    operator::OperatorErrorCode as PublicOperatorErrorCode,
};
use serde::Serialize;

use crate::{
    application::{
        command::CommandError,
        contest::ContestError,
        enrollment::EnrollmentError,
        import::{CsvImportErrorCategory, ImportError},
        operator::OperatorError,
        provisioning::ProvisioningError,
    },
    audit::CorrelationId,
};

#[derive(Serialize)]
struct ErrorResponse<'a> {
    title: &'static str,
    status: u16,
    code: &'static str,
    correlation_id: &'a str,
}

pub(super) struct ApiError {
    status: StatusCode,
    title: &'static str,
    code: &'static str,
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
            CommonErrorCode::AuthenticationFailed,
            cause,
            correlation_id,
        )
    }

    pub(super) fn authorization_denied(cause: &'static str, correlation_id: CorrelationId) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "Forbidden",
            CommonErrorCode::AuthorizationDenied,
            cause,
            correlation_id,
        )
    }

    pub(super) fn invalid_request(cause: &'static str, correlation_id: CorrelationId) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "Bad Request",
            CommonErrorCode::InvalidRequest,
            cause,
            correlation_id,
        )
    }

    pub(super) fn not_found(cause: &'static str, correlation_id: CorrelationId) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "Not Found",
            CommonErrorCode::ResourceNotFound,
            cause,
            correlation_id,
        )
    }

    pub(super) fn device_control_subprotocol_unsupported(correlation_id: CorrelationId) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            title: "Bad Request",
            code: ErrorCode::from(ControlErrorCode::ProtocolVersionUnsupported).as_str(),
            cause: "device_control_subprotocol_unsupported",
            correlation_id,
        }
    }

    pub(super) fn invalid_enrollment_request(
        cause: &'static str,
        correlation_id: CorrelationId,
    ) -> Self {
        Self::enrollment_error(
            StatusCode::BAD_REQUEST,
            "Bad Request",
            PublicEnrollmentErrorCode::EnrollmentRequestInvalid,
            cause,
            correlation_id,
        )
    }

    pub(super) fn from_enrollment(error: EnrollmentError, correlation_id: CorrelationId) -> Self {
        match error {
            EnrollmentError::InvalidRequestId => {
                Self::invalid_enrollment_request("enrollment_request_id_invalid", correlation_id)
            }
            EnrollmentError::InvalidMachineHardwareId => Self::invalid_enrollment_request(
                "enrollment_machine_hardware_id_invalid",
                correlation_id,
            ),
            EnrollmentError::InvalidHardwareIdentityQuality => Self::invalid_enrollment_request(
                "enrollment_hardware_identity_quality_invalid",
                correlation_id,
            ),
            EnrollmentError::InvalidClientVersion => Self::invalid_enrollment_request(
                "enrollment_client_version_invalid",
                correlation_id,
            ),
            EnrollmentError::UnsupportedProtocolVersion => Self::invalid_enrollment_request(
                "enrollment_protocol_version_unsupported",
                correlation_id,
            ),
            EnrollmentError::InvalidSpki => {
                Self::invalid_enrollment_request("enrollment_spki_invalid", correlation_id)
            }
            EnrollmentError::InvalidCsrEncoding => {
                Self::invalid_enrollment_request("enrollment_csr_encoding_invalid", correlation_id)
            }
            EnrollmentError::InvalidCsr => {
                Self::invalid_enrollment_request("enrollment_csr_invalid", correlation_id)
            }
            EnrollmentError::SpkiMismatch => {
                Self::invalid_enrollment_request("enrollment_spki_mismatch", correlation_id)
            }
            EnrollmentError::ProvisioningWindowClosed => Self::enrollment_error(
                StatusCode::CONFLICT,
                "Conflict",
                PublicEnrollmentErrorCode::ProvisioningWindowClosed,
                "enrollment_provisioning_window_closed",
                correlation_id,
            ),
            EnrollmentError::RequestRejected => Self::enrollment_error(
                StatusCode::CONFLICT,
                "Conflict",
                PublicEnrollmentErrorCode::EnrollmentRequestRejected,
                "enrollment_request_rejected",
                correlation_id,
            ),
            EnrollmentError::LiveRequestCapacityExceeded => Self::invalid_enrollment_request(
                "enrollment_live_request_capacity_exceeded",
                correlation_id,
            ),
            EnrollmentError::DeviceIdentityConflict => Self::enrollment_error(
                StatusCode::CONFLICT,
                "Conflict",
                PublicEnrollmentErrorCode::DeviceIdentityConflict,
                "enrollment_device_identity_conflict",
                correlation_id,
            ),
            EnrollmentError::RequestNotPending => Self::invalid_enrollment_request(
                "enrollment_request_not_actionable",
                correlation_id,
            ),
            EnrollmentError::InvalidPersistedFacts => {
                Self::internal_error("enrollment_invalid_persisted_facts", correlation_id)
            }
            EnrollmentError::EntropyUnavailable => {
                Self::internal_error("enrollment_entropy_unavailable", correlation_id)
            }
            EnrollmentError::IssuancePolicyExpired => {
                Self::internal_error("enrollment_issuance_policy_expired", correlation_id)
            }
            EnrollmentError::SigningFailed => {
                Self::internal_error("enrollment_signing_failed", correlation_id)
            }
            EnrollmentError::PersistenceFailed => {
                Self::internal_error("enrollment_persistence_failed", correlation_id)
            }
        }
    }

    pub(super) fn internal_error(cause: &'static str, correlation_id: CorrelationId) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            CommonErrorCode::InternalError,
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
            ContestError::InvalidDeviceId => {
                Self::invalid_request("contest_invalid_device_id", correlation_id)
            }
            ContestError::DeviceNotFound => {
                Self::not_found("contest_device_not_found", correlation_id)
            }
            ContestError::InvalidPersistedFacts => {
                Self::internal_error("contest_invalid_persisted_facts", correlation_id)
            }
            ContestError::PersistenceFailed => {
                Self::internal_error("contest_persistence_failed", correlation_id)
            }
        }
    }

    pub(super) fn from_command(error: CommandError, correlation_id: CorrelationId) -> Self {
        match error {
            CommandError::CommandIdInvalid => Self {
                status: StatusCode::BAD_REQUEST,
                title: "Bad Request",
                code: ErrorCode::from(ControlErrorCode::CommandIdInvalid).as_str(),
                cause: "command_id_invalid",
                correlation_id,
            },
            CommandError::RequestInvalid => {
                Self::invalid_request("command_request_invalid", correlation_id)
            }
            CommandError::DeviceIdInvalid => {
                Self::invalid_request("command_device_id_invalid", correlation_id)
            }
            CommandError::KindInvalid => {
                Self::invalid_request("command_kind_invalid", correlation_id)
            }
            CommandError::PayloadInvalid => {
                Self::invalid_request("command_payload_invalid", correlation_id)
            }
            CommandError::ReasonCodeInvalid => {
                Self::invalid_request("command_reason_code_invalid", correlation_id)
            }
            CommandError::GroupCorrelationIdInvalid => {
                Self::invalid_request("command_group_correlation_id_invalid", correlation_id)
            }
            CommandError::DeviceNotFound => {
                Self::not_found("command_device_not_found", correlation_id)
            }
            CommandError::RequestConflict => Self {
                status: StatusCode::CONFLICT,
                title: "Conflict",
                code: ErrorCode::from(ControlErrorCode::CommandRequestConflict).as_str(),
                cause: "command_request_conflict",
                correlation_id,
            },
            CommandError::CanonicalizationFailed => {
                Self::internal_error("command_canonicalization_failed", correlation_id)
            }
            CommandError::PersistenceFailed => {
                Self::internal_error("command_persistence_failed", correlation_id)
            }
        }
    }

    pub(super) fn from_import(error: ImportError, correlation_id: CorrelationId) -> Self {
        match error {
            ImportError::InvalidCsv(error) => {
                let cause = match error.category() {
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
                    PublicOperatorErrorCode::ImportCandidateInvalid,
                    cause,
                    correlation_id,
                )
            }
            ImportError::CandidateInvalid => Self::import_error(
                StatusCode::BAD_REQUEST,
                "Bad Request",
                PublicOperatorErrorCode::ImportCandidateInvalid,
                "import_candidate_invalid",
                correlation_id,
            ),
            ImportError::CandidatePending => Self::import_error(
                StatusCode::CONFLICT,
                "Conflict",
                PublicOperatorErrorCode::ImportCandidatePending,
                "import_candidate_pending",
                correlation_id,
            ),
            ImportError::CandidateUnavailable => Self::import_error(
                StatusCode::NOT_FOUND,
                "Not Found",
                PublicOperatorErrorCode::ImportCandidateUnavailable,
                "import_candidate_unavailable",
                correlation_id,
            ),
            ImportError::PreviewStale => Self::import_error(
                StatusCode::CONFLICT,
                "Conflict",
                PublicOperatorErrorCode::ImportPreviewStale,
                "import_preview_stale",
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
            ProvisioningError::RevisionOverflow => {
                Self::internal_error("provisioning_revision_overflow", correlation_id)
            }
            ProvisioningError::PersistenceFailed => {
                Self::internal_error("provisioning_persistence_failed", correlation_id)
            }
        }
    }

    fn import_error(
        status: StatusCode,
        title: &'static str,
        code: PublicOperatorErrorCode,
        cause: &'static str,
        correlation_id: CorrelationId,
    ) -> Self {
        Self {
            status,
            title,
            code: ErrorCode::from(code).as_str(),
            cause,
            correlation_id,
        }
    }

    fn enrollment_error(
        status: StatusCode,
        title: &'static str,
        code: PublicEnrollmentErrorCode,
        cause: &'static str,
        correlation_id: CorrelationId,
    ) -> Self {
        Self {
            status,
            title,
            code: ErrorCode::from(code).as_str(),
            cause,
            correlation_id,
        }
    }

    fn new(
        status: StatusCode,
        title: &'static str,
        code: CommonErrorCode,
        cause: &'static str,
        correlation_id: CorrelationId,
    ) -> Self {
        Self {
            status,
            title,
            code: ErrorCode::from(code).as_str(),
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
            code: self.code,
            correlation_id: &correlation_id,
        };
        let body = serde_json::to_vec(&error_response).unwrap_or_else(|_| {
            tracing::error!(
                code = CommonErrorCode::InternalError.as_str(),
                cause = "http_error_response_serialization_failed",
                correlation_id = %correlation_id,
                "HTTP error response serialization invariant failed"
            );
            panic!("HTTP error response serialization invariant failed");
        });

        if self.status.is_server_error() {
            tracing::error!(
                code = self.code,
                cause = self.cause,
                correlation_id = %correlation_id,
                "HTTP request failed"
            );
        } else {
            tracing::warn!(
                code = self.code,
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
