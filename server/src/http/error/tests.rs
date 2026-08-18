use axum::body::to_bytes;
use serde_json::Value;
use snafu::Snafu;
use tracing::instrument::WithSubscriber as _;
use uuid::Uuid;

use crate::{
    application::{
        command::CommandError,
        device::{DeviceError, enrollment::EnrollmentError},
        import::{ImportError, parse_csv},
        provisioning::ProvisioningError,
    },
    config::LogLevel,
    logging::tests::{CapturedLogs, SubscriberTestGuard},
};

use super::{ApiError, ContestError, CorrelationId, IntoResponse as _, OperatorError, StatusCode};

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
async fn from_command_maps_every_error_and_hides_non_enrollment_like_missing_device()
-> Result<(), TestFailure> {
    let correlation_id = CorrelationId::from_uuid(Uuid::now_v7());
    let mut missing_device_body = None;
    for (error, cause, expected_status, expected_code) in command_causes() {
        let response = ApiError::from_command(error, correlation_id).into_response();
        if response.status() != expected_status {
            return Err(TestFailure::CommandMappingChanged);
        }
        let body = to_bytes(response.into_body(), RESPONSE_BODY_LIMIT_BYTES)
            .await
            .map_err(|_| TestFailure::ResponseBodyWasNotReadable)?;
        let value: Value =
            serde_json::from_slice(&body).map_err(|_| TestFailure::ResponseBodyWasNotReadable)?;
        let object = value
            .as_object()
            .ok_or(TestFailure::CommandMappingChanged)?;
        if object.len() != 4
            || object.get("status").and_then(Value::as_u64)
                != Some(u64::from(expected_status.as_u16()))
            || object.get("code").and_then(Value::as_str) != Some(expected_code)
            || std::str::from_utf8(&body)
                .map_err(|_| TestFailure::ResponseBodyWasNotReadable)?
                .contains(cause)
        {
            return Err(TestFailure::CommandMappingChanged);
        }
        if error == CommandError::DeviceNotFound {
            missing_device_body = Some(body.clone());
        }
        if error == CommandError::DeviceNotEnrolled && missing_device_body.as_ref() != Some(&body) {
            return Err(TestFailure::CommandMappingChanged);
        }
    }
    Ok(())
}

#[tokio::test]
async fn the_internal_cause_is_logged_and_never_reaches_the_response() -> Result<(), TestFailure> {
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
        for (error, cause, status) in device_causes() {
            let rendered = ApiError::from_device(error, correlation_id);
            assert_cause_stays_internal(rendered, cause, status).await?;
            causes.push(cause);
        }
        for (error, cause, status, _code) in command_causes() {
            let rendered = ApiError::from_command(error, correlation_id);
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
    let body = std::str::from_utf8(&body).map_err(|_| TestFailure::ResponseBodyWasNotReadable)?;
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

fn contest_causes() -> [(ContestError, &'static str, StatusCode); 1] {
    [(
        ContestError::PersistenceFailed,
        "contest_persistence_failed",
        StatusCode::INTERNAL_SERVER_ERROR,
    )]
}

fn device_causes() -> [(DeviceError, &'static str, StatusCode); 4] {
    [
        (
            DeviceError::InvalidDeviceId,
            "contest_invalid_device_id",
            StatusCode::BAD_REQUEST,
        ),
        (
            DeviceError::DeviceNotFound,
            "contest_device_not_found",
            StatusCode::NOT_FOUND,
        ),
        (
            DeviceError::InvalidPersistedFacts,
            "contest_invalid_persisted_facts",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            DeviceError::PersistenceFailed,
            "contest_persistence_failed",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ]
}

fn command_causes() -> [(CommandError, &'static str, StatusCode, &'static str); 12] {
    [
        (
            CommandError::CommandIdInvalid,
            "command_id_invalid",
            StatusCode::BAD_REQUEST,
            "COMMAND_ID_INVALID",
        ),
        (
            CommandError::RequestInvalid,
            "command_request_invalid",
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
        ),
        (
            CommandError::DeviceIdInvalid,
            "command_device_id_invalid",
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
        ),
        (
            CommandError::KindInvalid,
            "command_kind_invalid",
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
        ),
        (
            CommandError::PayloadInvalid,
            "command_payload_invalid",
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
        ),
        (
            CommandError::ReasonCodeInvalid,
            "command_reason_code_invalid",
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
        ),
        (
            CommandError::GroupCorrelationIdInvalid,
            "command_group_correlation_id_invalid",
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
        ),
        (
            CommandError::DeviceNotFound,
            "command_device_not_found",
            StatusCode::NOT_FOUND,
            "RESOURCE_NOT_FOUND",
        ),
        (
            CommandError::DeviceNotEnrolled,
            "command_device_not_enrolled",
            StatusCode::NOT_FOUND,
            "RESOURCE_NOT_FOUND",
        ),
        (
            CommandError::RequestConflict,
            "command_request_conflict",
            StatusCode::CONFLICT,
            "COMMAND_REQUEST_CONFLICT",
        ),
        (
            CommandError::CanonicalizationFailed,
            "command_canonicalization_failed",
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
        ),
        (
            CommandError::PersistenceFailed,
            "command_persistence_failed",
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
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
    #[snafu(display("the exhaustive Command error mapping changed"))]
    CommandMappingChanged,
}
