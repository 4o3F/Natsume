use axum::{
    Extension, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::{
    application::{
        device::{self, DeviceError, DeviceId},
        operator::{self, OperatorIdentity},
    },
    audit::CorrelationId,
};

use super::super::super::{AppState, error::ApiError, middleware};

pub(super) fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/devices/{device_id}/actions/revoke",
            middleware::require_operator(state.clone(), post(revoke_device)),
        )
        .route(
            "/devices/{device_id}/actions/disable",
            middleware::require_operator(state, post(disable_device)),
        )
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct DevicePath {
    /// Canonical lowercase hyphenated `UUIDv7` Device ID.
    #[param(pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")]
    device_id: String,
}

#[utoipa::path(
    post,
    path = "/api/v2/devices/{device_id}/actions/revoke",
    operation_id = "revokeDevice",
    params(DevicePath),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Device revoke applied or already converged"),
        (status = 400, description = "Device ID is not canonical UUIDv7"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Device does not exist"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn revoke_device(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(identity): Extension<OperatorIdentity>,
    Path(path): Path<DevicePath>,
) -> Response {
    if let Err(error) = operator::require_admin(identity.role()) {
        return ApiError::from_operator(error, correlation_id).into_response();
    }
    let Some(device_id) = DeviceId::parse(&path.device_id) else {
        return ApiError::from_device(DeviceError::InvalidDeviceId, correlation_id).into_response();
    };
    match device::revoke_device(&state.database, &device_id, correlation_id).await {
        Ok(()) => {
            state.device_connections.evict(&device_id.as_text());
            StatusCode::OK.into_response()
        }
        Err(error) => ApiError::from_device(error, correlation_id).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/devices/{device_id}/actions/disable",
    operation_id = "disableDevice",
    params(DevicePath),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Device disable applied or already satisfied"),
        (status = 400, description = "Device ID is not canonical UUIDv7"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Device does not exist"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn disable_device(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(identity): Extension<OperatorIdentity>,
    Path(path): Path<DevicePath>,
) -> Response {
    if let Err(error) = operator::require_admin(identity.role()) {
        return ApiError::from_operator(error, correlation_id).into_response();
    }
    let Some(device_id) = DeviceId::parse(&path.device_id) else {
        return ApiError::from_device(DeviceError::InvalidDeviceId, correlation_id).into_response();
    };
    match device::disable_device(&state.database, &device_id, correlation_id).await {
        Ok(()) => {
            state.device_connections.evict(&device_id.as_text());
            StatusCode::OK.into_response()
        }
        Err(error) => ApiError::from_device(error, correlation_id).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode, header},
    };
    use serde_json::Value;
    use snafu::Snafu;
    use uuid::Uuid;

    use crate::{
        application::operator::{OperatorRole, sign_in, tests::PasswordVerificationTestGuard},
        audit::CorrelationId,
        db::{Database, device as db_device},
    };

    use super::super::super::super::{
        middleware::CORRELATION_ID_HEADER,
        router,
        tests::{
            Captured, SupportFailure, TestDatabase, drive, header_text, response_contains,
            seed_operator, unused_vault_master_key, unused_web_root,
        },
    };

    const PASSWORD: &str = "contest-read-password-canary";
    const ACTION_DEVICE: &str = "01900000-0000-7000-8000-000000000201";
    const SECOND_ACTION_DEVICE: &str = "01900000-0000-7000-8000-000000000202";
    const UNKNOWN_DEVICE: &str = "01900000-0000-7000-8000-000000000299";

    #[tokio::test]
    async fn admin_lifecycle_actions_are_wired_to_the_atomic_transitions() -> Result<(), TestFailure>
    {
        let fixture = TestDatabase::new().await?;
        for id in [ACTION_DEVICE, SECOND_ACTION_DEVICE] {
            db_device::tests::test_seed_lifecycle_device(
                &fixture.database,
                id,
                "enrolled",
                true,
                "active",
            )
            .await
            .map_err(|_| TestFailure::FixtureFailed)?;
        }
        seed_contest_operator(&fixture.database, "action-admin", OperatorRole::Admin).await?;
        let cookie = session_cookie(&fixture.database, "action-admin").await?;
        let application = router(
            fixture.database.clone(),
            unused_vault_master_key(),
            unused_web_root(),
        );

        let revoke = drive(
            &application,
            action_request(
                &format!("/api/v2/devices/{ACTION_DEVICE}/actions/revoke"),
                Some(&cookie),
            )?,
        )
        .await?;
        let disable = drive(
            &application,
            action_request(
                &format!("/api/v2/devices/{SECOND_ACTION_DEVICE}/actions/disable"),
                Some(&cookie),
            )?,
        )
        .await?;
        let revoked = db_device::tests::test_lifecycle_snapshot(&fixture.database, ACTION_DEVICE)
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let disabled =
            db_device::tests::test_lifecycle_snapshot(&fixture.database, SECOND_ACTION_DEVICE)
                .await
                .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        if revoke.status != StatusCode::OK
            || !revoke.body.is_empty()
            || disable.status != StatusCode::OK
            || !disable.body.is_empty()
            || revoked.state != "revoked"
            || revoked.token_count != 0
            || revoked.certificate_statuses != ["revoked"]
            || disabled.state != "disabled"
            || disabled.token_count != 1
            || disabled.certificate_statuses != ["active"]
        {
            return Err(TestFailure::LifecycleActionChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn lifecycle_boundary_has_no_auth_or_device_existence_oracle() -> Result<(), TestFailure>
    {
        let fixture = TestDatabase::new().await?;
        db_device::tests::test_seed_lifecycle_device(
            &fixture.database,
            ACTION_DEVICE,
            "enrolled",
            true,
            "active",
        )
        .await
        .map_err(|_| TestFailure::FixtureFailed)?;
        seed_contest_operator(&fixture.database, "boundary-admin", OperatorRole::Admin).await?;
        seed_contest_operator(&fixture.database, "boundary-viewer", OperatorRole::Viewer).await?;
        let admin_cookie = session_cookie(&fixture.database, "boundary-admin").await?;
        let viewer_cookie = session_cookie(&fixture.database, "boundary-viewer").await?;
        let application = router(
            fixture.database.clone(),
            unused_vault_master_key(),
            unused_web_root(),
        );
        let before = db_device::tests::test_lifecycle_snapshot(&fixture.database, ACTION_DEVICE)
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;

        let existing_viewer = drive(
            &application,
            action_request(
                &format!("/api/v2/devices/{ACTION_DEVICE}/actions/revoke"),
                Some(&viewer_cookie),
            )?,
        )
        .await?;
        let unknown_viewer = drive(
            &application,
            action_request(
                &format!("/api/v2/devices/{UNKNOWN_DEVICE}/actions/revoke"),
                Some(&viewer_cookie),
            )?,
        )
        .await?;
        let existing_error_response = normalized_error_response(
            &existing_viewer,
            StatusCode::FORBIDDEN,
            "Forbidden",
            "AUTHORIZATION_DENIED",
        )?;
        let unknown_error_response = normalized_error_response(
            &unknown_viewer,
            StatusCode::FORBIDDEN,
            "Forbidden",
            "AUTHORIZATION_DENIED",
        )?;
        let after_viewer =
            db_device::tests::test_lifecycle_snapshot(&fixture.database, ACTION_DEVICE)
                .await
                .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        if existing_error_response != unknown_error_response || before != after_viewer {
            return Err(TestFailure::DeviceExistenceOracleOpened);
        }

        let unauthenticated = drive(
            &application,
            action_request(
                &format!("/api/v2/devices/{ACTION_DEVICE}/actions/revoke"),
                None,
            )?,
        )
        .await?;
        let _normalized = normalized_authentication_error_response(&unauthenticated)?;

        let canary = "NOT-CANONICAL-DEVICE-ID-SECRET-CANARY";
        let invalid = drive(
            &application,
            action_request(
                &format!("/api/v2/devices/{canary}/actions/disable"),
                Some(&admin_cookie),
            )?,
        )
        .await?;
        let _normalized = normalized_error_response(
            &invalid,
            StatusCode::BAD_REQUEST,
            "Bad Request",
            "INVALID_REQUEST",
        )?;
        if response_contains(&invalid, canary) {
            return Err(TestFailure::InvalidDeviceIdEscaped);
        }

        let unknown = drive(
            &application,
            action_request(
                &format!("/api/v2/devices/{UNKNOWN_DEVICE}/actions/disable"),
                Some(&admin_cookie),
            )?,
        )
        .await?;
        let _normalized = normalized_error_response(
            &unknown,
            StatusCode::NOT_FOUND,
            "Not Found",
            "RESOURCE_NOT_FOUND",
        )?;
        Ok(())
    }

    async fn seed_contest_operator(
        database: &Database,
        login_name: &str,
        role: OperatorRole,
    ) -> Result<(), TestFailure> {
        seed_operator(database, login_name, role, PASSWORD).await?;
        Ok(())
    }

    async fn session_cookie(database: &Database, login_name: &str) -> Result<String, TestFailure> {
        let _verification_guard = PasswordVerificationTestGuard::acquire().await;
        let session = sign_in(
            database,
            CorrelationId::from_uuid(Uuid::now_v7()),
            login_name,
            PASSWORD.to_owned(),
        )
        .await
        .map_err(|_| TestFailure::FixtureFailed)?;
        Ok(format!(
            "__Secure-natsume_session={}",
            session.credential().to_wire().expose()
        ))
    }

    fn action_request(path: &str, cookie: Option<&str>) -> Result<Request<Body>, TestFailure> {
        let mut request = Request::builder().method(Method::POST).uri(path);
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        request
            .body(Body::empty())
            .map_err(|_| TestFailure::RequestBuildFailed)
    }

    fn normalized_authentication_error_response(
        response: &Captured,
    ) -> Result<String, TestFailure> {
        normalized_error_response(
            response,
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            "AUTHENTICATION_FAILED",
        )
        .map_err(|_| TestFailure::AuthenticationErrorResponseChanged)
    }

    fn normalized_error_response(
        response: &Captured,
        status: StatusCode,
        title: &str,
        code: &str,
    ) -> Result<String, TestFailure> {
        if response.status != status
            || header_text(&response.headers, &header::CONTENT_TYPE)? != "application/json"
        {
            return Err(TestFailure::ErrorResponseChanged);
        }
        let correlation = header_text(&response.headers, &CORRELATION_ID_HEADER)?;
        let parsed = Uuid::parse_str(correlation).map_err(|_| TestFailure::ErrorResponseChanged)?;
        let mut value: Value =
            serde_json::from_slice(&response.body).map_err(|_| TestFailure::ResponseJsonInvalid)?;
        let object = value
            .as_object_mut()
            .ok_or(TestFailure::ErrorResponseChanged)?;
        if parsed.get_version_num() != 7
            || object.len() != 4
            || object.get("title").and_then(Value::as_str) != Some(title)
            || object.get("status").and_then(Value::as_u64) != Some(u64::from(status.as_u16()))
            || object.get("code").and_then(Value::as_str) != Some(code)
            || object.get("correlation_id").and_then(Value::as_str) != Some(correlation)
        {
            return Err(TestFailure::ErrorResponseChanged);
        }
        object.insert(
            "correlation_id".to_owned(),
            Value::String("NORMALIZED".to_owned()),
        );
        serde_json::to_string(&value).map_err(|_| TestFailure::ResponseJsonInvalid)
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("an HTTP test helper failed"))]
        #[snafu(context(false))]
        Support { source: SupportFailure },
        #[snafu(display("the contest HTTP fixture failed"))]
        FixtureFailed,
        #[snafu(display("the contest HTTP request could not be built"))]
        RequestBuildFailed,
        #[snafu(display("a contest HTTP response was not valid JSON"))]
        ResponseJsonInvalid,
        #[snafu(display("a contest authentication error response changed"))]
        AuthenticationErrorResponseChanged,
        #[snafu(display("contest database evidence could not be read"))]
        DatabaseEvidenceFailed,
        #[snafu(display("a Device lifecycle HTTP action changed"))]
        LifecycleActionChanged,
        #[snafu(display("the Device lifecycle boundary exposed an existence oracle"))]
        DeviceExistenceOracleOpened,
        #[snafu(display("a Device lifecycle error response changed"))]
        ErrorResponseChanged,
        #[snafu(display("an invalid Device ID escaped into the response"))]
        InvalidDeviceIdEscaped,
    }
}
