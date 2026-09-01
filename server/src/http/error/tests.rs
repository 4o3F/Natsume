use axum::body::to_bytes;
use snafu::Snafu;
use tracing::instrument::WithSubscriber as _;

use crate::{
    component::{
        import::{CsvImportErrorCategory, ImportError},
        lifecycle::DeviceError,
    },
    config::LogLevel,
    logging::tests::{CapturedLogs, SubscriberTestGuard},
};

use super::{ApiError, ContestError, IntoResponse as _, OperatorError, StatusCode};

const CAUSE_CANARY: &str = "internal_cause_canary";
const RESPONSE_BODY_LIMIT_BYTES: usize = 4 * 1024;

#[tokio::test]
async fn the_internal_cause_is_logged_and_never_reaches_the_response() -> Result<(), TestFailure> {
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let captured = CapturedLogs::default();
    let subscriber = captured.subscriber(LogLevel::Trace);
    let causes = async {
        let mut causes = Vec::new();
        for (error, cause, status) in operator_causes() {
            let rendered = ApiError::from_operator(error);
            assert_cause_stays_internal(rendered, cause, status).await?;
            causes.push(cause);
        }
        for (error, cause, status) in contest_causes() {
            let rendered = ApiError::from_contest(error);
            assert_cause_stays_internal(rendered, cause, status).await?;
            causes.push(cause);
        }
        for (error, cause, status) in device_causes() {
            let rendered = ApiError::from_device(error);
            assert_cause_stays_internal(rendered, cause, status).await?;
            causes.push(cause);
        }
        for (error, cause, status) in import_causes() {
            let rendered = ApiError::from_import(error);
            assert_cause_stays_internal(rendered, cause, status).await?;
            causes.push(cause);
        }
        // The failures with no typed error to match on carry a caller-chosen
        // static cause; the canary proves it stays out of the response.
        for (error, status) in [
            (
                ApiError::authentication_failed(CAUSE_CANARY),
                StatusCode::UNAUTHORIZED,
            ),
            (
                ApiError::authorization_denied(CAUSE_CANARY),
                StatusCode::FORBIDDEN,
            ),
            (
                ApiError::invalid_request(CAUSE_CANARY),
                StatusCode::BAD_REQUEST,
            ),
            (ApiError::not_found(CAUSE_CANARY), StatusCode::NOT_FOUND),
            (
                ApiError::internal_error(CAUSE_CANARY),
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

fn import_causes() -> [(ImportError, &'static str, StatusCode); 8] {
    [
        (
            ImportError::InvalidCsv(CsvImportErrorCategory::ZeroDataRows),
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
