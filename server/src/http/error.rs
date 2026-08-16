use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use natsume_error_code::{
    ErrorCode, common::CommonErrorCode,
    enrollment::EnrollmentErrorCode as PublicEnrollmentErrorCode,
    operator::OperatorErrorCode as PublicOperatorErrorCode,
};
use serde::Serialize;

use crate::{
    application::{
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
mod tests {
    use axum::body::to_bytes;
    use serde_json::Value;
    use snafu::Snafu;
    use tracing::instrument::WithSubscriber as _;
    use uuid::Uuid;

    use crate::{
        application::{
            enrollment::EnrollmentError,
            import::{ImportError, parse_csv},
            provisioning::ProvisioningError,
        },
        config::LogLevel,
        logging::tests::{CapturedLogs, SubscriberTestGuard},
    };

    use super::{
        ApiError, ContestError, CorrelationId, IntoResponse as _, OperatorError, StatusCode,
    };

    const CAUSE_CANARY: &str = "internal_cause_canary";
    const RESPONSE_BODY_LIMIT_BYTES: usize = 4 * 1024;
    const INVALID_ENROLLMENT_REQUEST_ID_CAUSE: (EnrollmentError, &str, StatusCode) = (
        EnrollmentError::InvalidRequestId,
        "enrollment_request_id_invalid",
        StatusCode::BAD_REQUEST,
    );

    #[tokio::test]
    async fn non_pending_enrollment_decision_is_invalid_not_identity_conflict()
    -> Result<(), TestFailure> {
        let response = ApiError::from_enrollment(
            EnrollmentError::RequestNotPending,
            CorrelationId::from_uuid(Uuid::now_v7()),
        )
        .into_response();
        if response.status() != StatusCode::BAD_REQUEST {
            return Err(TestFailure::EnrollmentDecisionMappingChanged);
        }
        let body = to_bytes(response.into_body(), RESPONSE_BODY_LIMIT_BYTES)
            .await
            .map_err(|_| TestFailure::ResponseBodyWasNotReadable)?;
        let body: Value =
            serde_json::from_slice(&body).map_err(|_| TestFailure::ResponseBodyWasNotReadable)?;
        if body.get("code").and_then(Value::as_str) != Some("ENROLLMENT_REQUEST_INVALID") {
            return Err(TestFailure::EnrollmentDecisionMappingChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn the_internal_cause_is_logged_and_never_reaches_the_response() -> Result<(), TestFailure>
    {
        let _subscriber_guard = SubscriberTestGuard::acquire();
        let captured = CapturedLogs::default();
        let subscriber = captured.subscriber(LogLevel::Trace);
        let correlation_id = CorrelationId::from_uuid(Uuid::now_v7());
        let causes = async {
            let mut causes = Vec::new();
            for (error, cause, status) in operator_causes() {
                let rendered = ApiError::from_operator(error, correlation_id);
                assert_cause_stays_internal(rendered, cause, status).await?;
                causes.push(cause);
            }
            for (error, cause, status) in contest_causes() {
                let rendered = ApiError::from_contest(error, correlation_id);
                assert_cause_stays_internal(rendered, cause, status).await?;
                causes.push(cause);
            }
            for (error, cause, status) in import_causes() {
                let rendered = ApiError::from_import(error, correlation_id);
                assert_cause_stays_internal(rendered, cause, status).await?;
                causes.push(cause);
            }
            for (error, cause, status) in provisioning_causes() {
                let rendered = ApiError::from_provisioning(error, correlation_id);
                assert_cause_stays_internal(rendered, cause, status).await?;
                causes.push(cause);
            }
            for (error, cause, status) in enrollment_causes() {
                let rendered = ApiError::from_enrollment(error, correlation_id);
                assert_cause_stays_internal(rendered, cause, status).await?;
                causes.push(cause);
            }
            // The failures with no typed error to match on carry a caller-chosen
            // static cause; the canary proves it stays out of the response.
            for (error, status) in [
                (
                    ApiError::authentication_failed(CAUSE_CANARY, correlation_id),
                    StatusCode::UNAUTHORIZED,
                ),
                (
                    ApiError::authorization_denied(CAUSE_CANARY, correlation_id),
                    StatusCode::FORBIDDEN,
                ),
                (
                    ApiError::invalid_request(CAUSE_CANARY, correlation_id),
                    StatusCode::BAD_REQUEST,
                ),
                (
                    ApiError::not_found(CAUSE_CANARY, correlation_id),
                    StatusCode::NOT_FOUND,
                ),
                (
                    ApiError::internal_error(CAUSE_CANARY, correlation_id),
                    StatusCode::INTERNAL_SERVER_ERROR,
                ),
            ] {
                assert_cause_stays_internal(error, CAUSE_CANARY, status).await?;
                causes.push(CAUSE_CANARY);
            }
            Ok::<_, TestFailure>(causes)
        }
        .with_subscriber(subscriber)
        .await?;

        let output = captured
            .text()
            .map_err(|()| TestFailure::LogCaptureFailed)?;
        if output.matches("cause=").count() != causes.len() {
            return Err(TestFailure::CauseWasNotLogged);
        }
        for cause in causes {
            if !output.contains(&format!("cause=\"{cause}\"")) {
                return Err(TestFailure::CauseWasNotLogged);
            }
            // A compile-time constant discriminant, never request-derived text.
            if cause.is_empty()
                || !cause
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            {
                return Err(TestFailure::CauseWasNotAStaticDiscriminant);
            }
        }
        Ok(())
    }

    async fn assert_cause_stays_internal(
        error: ApiError,
        cause: &str,
        expected_status: StatusCode,
    ) -> Result<(), TestFailure> {
        let response = error.into_response();
        if response.status() != expected_status {
            return Err(TestFailure::PublishedStatusChanged);
        }
        if response
            .headers()
            .values()
            .any(|value| value.to_str().is_ok_and(|value| value.contains(cause)))
        {
            return Err(TestFailure::CauseEscapedIntoTheResponse);
        }
        let body = to_bytes(response.into_body(), RESPONSE_BODY_LIMIT_BYTES)
            .await
            .map_err(|_| TestFailure::ResponseBodyWasNotReadable)?;
        let body =
            std::str::from_utf8(&body).map_err(|_| TestFailure::ResponseBodyWasNotReadable)?;
        if body.contains(cause) {
            return Err(TestFailure::CauseEscapedIntoTheResponse);
        }
        Ok(())
    }

    fn operator_causes() -> [(OperatorError, &'static str, StatusCode); 15] {
        [
            (
                OperatorError::AuthenticationFailed,
                "operator_authentication_failed",
                StatusCode::UNAUTHORIZED,
            ),
            (
                OperatorError::SessionAuthenticationFailed,
                "operator_session_authentication_failed",
                StatusCode::UNAUTHORIZED,
            ),
            (
                OperatorError::InvalidSessionCredential,
                "operator_invalid_session_credential",
                StatusCode::UNAUTHORIZED,
            ),
            (
                OperatorError::AuthorizationDenied,
                "operator_authorization_denied",
                StatusCode::FORBIDDEN,
            ),
            (
                OperatorError::PersistenceFailed,
                "operator_persistence_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                OperatorError::PasswordTaskFailed,
                "operator_password_task_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                OperatorError::PasswordVerificationFailed,
                "operator_password_verification_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                OperatorError::InvalidPersistedIdentity,
                "operator_invalid_persisted_identity",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                OperatorError::InvalidPersistedRole,
                "operator_invalid_persisted_role",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                OperatorError::EntropyUnavailable,
                "operator_entropy_unavailable",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                OperatorError::InvalidHashingParameters,
                "operator_invalid_hashing_parameters",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                OperatorError::SaltEncodingFailed,
                "operator_salt_encoding_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                OperatorError::PasswordHashingFailed,
                "operator_password_hashing_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                OperatorError::EmptyLoginName,
                "operator_empty_login_name",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                OperatorError::PasswordMismatch,
                "operator_password_mismatch",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ]
    }

    fn provisioning_causes() -> [(ProvisioningError, &'static str, StatusCode); 2] {
        [
            (
                ProvisioningError::RevisionOverflow,
                "provisioning_revision_overflow",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                ProvisioningError::PersistenceFailed,
                "provisioning_persistence_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ]
    }

    fn enrollment_causes() -> [(EnrollmentError, &'static str, StatusCode); 19] {
        [
            INVALID_ENROLLMENT_REQUEST_ID_CAUSE,
            (
                EnrollmentError::InvalidMachineHardwareId,
                "enrollment_machine_hardware_id_invalid",
                StatusCode::BAD_REQUEST,
            ),
            (
                EnrollmentError::InvalidHardwareIdentityQuality,
                "enrollment_hardware_identity_quality_invalid",
                StatusCode::BAD_REQUEST,
            ),
            (
                EnrollmentError::InvalidClientVersion,
                "enrollment_client_version_invalid",
                StatusCode::BAD_REQUEST,
            ),
            (
                EnrollmentError::UnsupportedProtocolVersion,
                "enrollment_protocol_version_unsupported",
                StatusCode::BAD_REQUEST,
            ),
            (
                EnrollmentError::InvalidSpki,
                "enrollment_spki_invalid",
                StatusCode::BAD_REQUEST,
            ),
            (
                EnrollmentError::InvalidCsrEncoding,
                "enrollment_csr_encoding_invalid",
                StatusCode::BAD_REQUEST,
            ),
            (
                EnrollmentError::InvalidCsr,
                "enrollment_csr_invalid",
                StatusCode::BAD_REQUEST,
            ),
            (
                EnrollmentError::SpkiMismatch,
                "enrollment_spki_mismatch",
                StatusCode::BAD_REQUEST,
            ),
            (
                EnrollmentError::ProvisioningWindowClosed,
                "enrollment_provisioning_window_closed",
                StatusCode::CONFLICT,
            ),
            (
                EnrollmentError::RequestRejected,
                "enrollment_request_rejected",
                StatusCode::CONFLICT,
            ),
            (
                EnrollmentError::LiveRequestCapacityExceeded,
                "enrollment_live_request_capacity_exceeded",
                StatusCode::BAD_REQUEST,
            ),
            (
                EnrollmentError::DeviceIdentityConflict,
                "enrollment_device_identity_conflict",
                StatusCode::CONFLICT,
            ),
            (
                EnrollmentError::RequestNotPending,
                "enrollment_request_not_actionable",
                StatusCode::BAD_REQUEST,
            ),
            (
                EnrollmentError::InvalidPersistedFacts,
                "enrollment_invalid_persisted_facts",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                EnrollmentError::EntropyUnavailable,
                "enrollment_entropy_unavailable",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                EnrollmentError::IssuancePolicyExpired,
                "enrollment_issuance_policy_expired",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                EnrollmentError::SigningFailed,
                "enrollment_signing_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                EnrollmentError::PersistenceFailed,
                "enrollment_persistence_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ]
    }

    fn contest_causes() -> [(ContestError, &'static str, StatusCode); 4] {
        [
            (
                ContestError::InvalidDeviceId,
                "contest_invalid_device_id",
                StatusCode::BAD_REQUEST,
            ),
            (
                ContestError::DeviceNotFound,
                "contest_device_not_found",
                StatusCode::NOT_FOUND,
            ),
            (
                ContestError::InvalidPersistedFacts,
                "contest_invalid_persisted_facts",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                ContestError::PersistenceFailed,
                "contest_persistence_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ]
    }

    fn import_causes() -> [(ImportError, &'static str, StatusCode); 8] {
        let Err(parse_error) = parse_csv(&[]) else {
            panic!("empty import unexpectedly parsed");
        };
        [
            (
                ImportError::InvalidCsv(parse_error),
                "import_csv_zero_data_rows",
                StatusCode::BAD_REQUEST,
            ),
            (
                ImportError::CandidateInvalid,
                "import_candidate_invalid",
                StatusCode::BAD_REQUEST,
            ),
            (
                ImportError::CandidatePending,
                "import_candidate_pending",
                StatusCode::CONFLICT,
            ),
            (
                ImportError::CandidateUnavailable,
                "import_candidate_unavailable",
                StatusCode::NOT_FOUND,
            ),
            (
                ImportError::PreviewStale,
                "import_preview_stale",
                StatusCode::CONFLICT,
            ),
            (
                ImportError::EntropyUnavailable,
                "import_entropy_unavailable",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                ImportError::VaultFailure,
                "import_vault_failure",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                ImportError::PersistenceFailure,
                "import_persistence_failure",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ]
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("captured HTTP error logs could not be read"))]
        LogCaptureFailed,
        #[snafu(display("an HTTP error response body could not be read"))]
        ResponseBodyWasNotReadable,
        #[snafu(display("the published HTTP error status changed"))]
        PublishedStatusChanged,
        #[snafu(display("the internal failure cause escaped into the HTTP response"))]
        CauseEscapedIntoTheResponse,
        #[snafu(display("the internal failure cause was not logged"))]
        CauseWasNotLogged,
        #[snafu(display("an internal failure cause was not a static discriminant"))]
        CauseWasNotAStaticDiscriminant,
        #[snafu(display("the non-pending Enrollment decision mapping changed"))]
        EnrollmentDecisionMappingChanged,
    }
}
