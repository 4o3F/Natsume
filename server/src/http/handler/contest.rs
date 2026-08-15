use axum::{
    Extension, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};
use utoipa::IntoParams;

use crate::{
    application::{
        contest::{self, AccountFacts, BindingFacts, DeviceFacts, DeviceId, SeatFacts},
        operator::{self, OperatorIdentity},
    },
    audit::CorrelationId,
};

use super::super::{AppState, error::ApiError, middleware};

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/seats",
            middleware::operator_get(state.clone(), list_seats),
        )
        .route(
            "/accounts",
            middleware::operator_get(state.clone(), list_accounts),
        )
        .route(
            "/devices",
            middleware::operator_get(state.clone(), list_devices),
        )
        .route(
            "/bindings",
            middleware::operator_get(state.clone(), list_bindings),
        )
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
    get,
    path = "/api/v2/seats",
    operation_id = "listSeats",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current Seat set", body = [SeatFacts]),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn list_seats(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(_identity): Extension<OperatorIdentity>,
) -> Response {
    match contest::list_seats(&state.database).await {
        Ok(facts) => current_facts_response(&facts, correlation_id),
        Err(error) => ApiError::from_contest(error, correlation_id).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/accounts",
    operation_id = "listAccounts",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current Account set", body = [AccountFacts]),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn list_accounts(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(_identity): Extension<OperatorIdentity>,
) -> Response {
    match contest::list_accounts(&state.database).await {
        Ok(facts) => current_facts_response(&facts, correlation_id),
        Err(error) => ApiError::from_contest(error, correlation_id).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/devices",
    operation_id = "listDevices",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current Device set", body = [DeviceFacts]),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn list_devices(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(_identity): Extension<OperatorIdentity>,
) -> Response {
    match contest::list_devices(&state.database).await {
        Ok(facts) => current_facts_response(&facts, correlation_id),
        Err(error) => ApiError::from_contest(error, correlation_id).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/bindings",
    operation_id = "listBindings",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current Seat-to-Device Binding set", body = [BindingFacts]),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn list_bindings(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(_identity): Extension<OperatorIdentity>,
) -> Response {
    match contest::list_bindings(&state.database).await {
        Ok(facts) => current_facts_response(&facts, correlation_id),
        Err(error) => ApiError::from_contest(error, correlation_id).into_response(),
    }
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
    let device_id = match DeviceId::parse(&path.device_id) {
        Ok(device_id) => device_id,
        Err(error) => return ApiError::from_contest(error, correlation_id).into_response(),
    };
    match contest::revoke_device(&state.database, &device_id, correlation_id).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(error) => ApiError::from_contest(error, correlation_id).into_response(),
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
    let device_id = match DeviceId::parse(&path.device_id) {
        Ok(device_id) => device_id,
        Err(error) => return ApiError::from_contest(error, correlation_id).into_response(),
    };
    match contest::disable_device(&state.database, &device_id, correlation_id).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(error) => ApiError::from_contest(error, correlation_id).into_response(),
    }
}

fn current_facts_response<T: Serialize>(facts: &[T], correlation_id: CorrelationId) -> Response {
    let body = serde_json::to_string(&facts).unwrap_or_else(|_| {
        tracing::error!(
            correlation_id = %correlation_id.as_text(),
            "current facts response serialization invariant failed"
        );
        panic!("current facts response serialization invariant failed");
    });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
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
        db::{Database, contest as db_contest},
    };

    use super::super::super::{
        middleware::CORRELATION_ID_HEADER,
        router,
        tests::{
            Captured, SupportFailure, TestDatabase, drive, header_text, response_contains,
            seed_operator, unused_web_root,
        },
    };

    const PASSWORD: &str = "contest-read-password-canary";
    const VAULT_POINTER_CANARY: &str = "vault-pointer-secret-storage-canary";
    const HARDWARE_ID_CANARY: &str = "machine-hardware-id-full-canary-7d58f1";
    const ACTION_DEVICE: &str = "01900000-0000-7000-8000-000000000201";
    const SECOND_ACTION_DEVICE: &str = "01900000-0000-7000-8000-000000000202";
    const UNKNOWN_DEVICE: &str = "01900000-0000-7000-8000-000000000299";
    const ROUTES: [&str; 4] = [
        "/api/v2/seats",
        "/api/v2/accounts",
        "/api/v2/devices",
        "/api/v2/bindings",
    ];

    #[tokio::test]
    async fn admin_and_viewer_read_exact_redacted_current_facts_without_writes()
    -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        db_contest::tests::test_seed_current_facts(
            &fixture.database,
            VAULT_POINTER_CANARY,
            HARDWARE_ID_CANARY,
        )
        .await
        .map_err(|_| TestFailure::FixtureFailed)?;
        seed_contest_operator(&fixture.database, "contest-admin", OperatorRole::Admin).await?;
        seed_contest_operator(&fixture.database, "contest-viewer", OperatorRole::Viewer).await?;
        let admin_cookie = session_cookie(&fixture.database, "contest-admin").await?;
        let viewer_cookie = session_cookie(&fixture.database, "contest-viewer").await?;
        let application = router(fixture.database.clone(), unused_web_root());
        let mut observer = db_contest::tests::test_observer(&fixture.path)
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let before = db_contest::tests::test_snapshot(&fixture.database, &mut observer)
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;

        for (path, expected) in expected_current_facts() {
            let admin = drive(&application, request(path, Some(&admin_cookie))?).await?;
            let viewer = drive(&application, request(path, Some(&viewer_cookie))?).await?;
            verify_current_facts(path, &admin, &viewer, &expected)?;
        }

        let after = db_contest::tests::test_snapshot(&fixture.database, &mut observer)
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        if after != before {
            return Err(TestFailure::ReadChangedPersistence);
        }
        Ok(())
    }

    #[tokio::test]
    async fn empty_current_fact_sets_return_empty_arrays() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        seed_contest_operator(&fixture.database, "empty-admin", OperatorRole::Admin).await?;
        let cookie = session_cookie(&fixture.database, "empty-admin").await?;
        let application = router(fixture.database.clone(), unused_web_root());
        for path in ROUTES {
            let response = drive(&application, request(path, Some(&cookie))?).await?;
            if response.status != StatusCode::OK
                || response.body != b"[]"
                || header_text(&response.headers, &header::CONTENT_TYPE)? != "application/json"
            {
                return Err(TestFailure::EmptySetContractChanged);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn every_read_route_unifies_all_session_credential_failures() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        seed_contest_operator(&fixture.database, "failure-admin", OperatorRole::Admin).await?;
        let mut expired_cookies = Vec::with_capacity(ROUTES.len());
        for _path in ROUTES {
            expired_cookies.push(session_cookie(&fixture.database, "failure-admin").await?);
        }
        db_contest::tests::test_expire_all_sessions(&fixture.database)
            .await
            .map_err(|_| TestFailure::FixtureFailed)?;
        let application = router(fixture.database.clone(), unused_web_root());
        let mut expected = None;

        for (path, expired_cookie) in ROUTES.into_iter().zip(expired_cookies) {
            for cookie in [
                None,
                Some("__Secure-natsume_session=NOT-LOWERCASE-HEX"),
                Some(concat!(
                    "__Secure-natsume_session=",
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                )),
                Some(expired_cookie.as_str()),
            ] {
                let response = drive(&application, request(path, cookie)?).await?;
                let normalized = normalized_authentication_error_response(&response)?;
                if expected
                    .as_ref()
                    .is_some_and(|expected| expected != &normalized)
                {
                    return Err(TestFailure::AuthenticationFailuresDiverged);
                }
                expected.get_or_insert(normalized);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn admin_lifecycle_actions_are_wired_to_the_atomic_transitions() -> Result<(), TestFailure>
    {
        let fixture = TestDatabase::new().await?;
        for id in [ACTION_DEVICE, SECOND_ACTION_DEVICE] {
            db_contest::tests::test_seed_lifecycle_device(
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
        let application = router(fixture.database.clone(), unused_web_root());

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
        let revoked = db_contest::tests::test_lifecycle_snapshot(&fixture.database, ACTION_DEVICE)
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let disabled =
            db_contest::tests::test_lifecycle_snapshot(&fixture.database, SECOND_ACTION_DEVICE)
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
        db_contest::tests::test_seed_lifecycle_device(
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
        let application = router(fixture.database.clone(), unused_web_root());
        let before = db_contest::tests::test_lifecycle_snapshot(&fixture.database, ACTION_DEVICE)
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
            db_contest::tests::test_lifecycle_snapshot(&fixture.database, ACTION_DEVICE)
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
            "INVALID_REQUEST",
        )?;
        Ok(())
    }

    fn expected_current_facts() -> [(&'static str, Value); 4] {
        [
            (
                "/api/v2/seats",
                serde_json::json!([
                    {"seat_id":"seat-a","seat_code":"A-01"},
                    {"seat_id":"seat-b","seat_code":"B-02"}
                ]),
            ),
            (
                "/api/v2/accounts",
                serde_json::json!([
                    {"account_id":"account-a","domjudge_username":"team-alpha","credential_revision":3},
                    {"account_id":"account-b","domjudge_username":"team-beta","credential_revision":7}
                ]),
            ),
            (
                "/api/v2/devices",
                serde_json::json!([
                    {"device_id":"device-a","state":"enrolled","hardware_identity_quality":"strong"},
                    {"device_id":"device-b","state":"disabled","hardware_identity_quality":"medium"}
                ]),
            ),
            (
                "/api/v2/bindings",
                serde_json::json!([
                    {"seat_id":"seat-a","device_id":"device-a","binding_revision":11},
                    {"seat_id":"seat-b","device_id":"device-b","binding_revision":11}
                ]),
            ),
        ]
    }

    fn verify_current_facts(
        path: &str,
        admin: &Captured,
        viewer: &Captured,
        expected: &Value,
    ) -> Result<(), TestFailure> {
        if admin.status != StatusCode::OK
            || viewer.status != StatusCode::OK
            || admin.body != viewer.body
            || header_text(&admin.headers, &header::CONTENT_TYPE)? != "application/json"
            || serde_json::from_slice::<Value>(&admin.body)
                .map_err(|_| TestFailure::ResponseJsonInvalid)?
                != *expected
        {
            return Err(TestFailure::CurrentFactsChanged);
        }
        let encoded =
            std::str::from_utf8(&admin.body).map_err(|_| TestFailure::ResponseBodyFailed)?;
        if encoded.contains(VAULT_POINTER_CANARY)
            || encoded.contains(HARDWARE_ID_CANARY)
            || encoded.contains("credential_vault_record_id")
            || encoded.contains("machine_hardware_id")
            || encoded.to_ascii_lowercase().contains("password")
            || (!ROUTES.contains(&path) || expected.as_array().is_none())
        {
            return Err(TestFailure::RedactedFactEscaped);
        }
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

    fn request(path: &str, cookie: Option<&str>) -> Result<Request<Body>, TestFailure> {
        let mut request = Request::builder().method(Method::GET).uri(path);
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        request
            .body(Body::empty())
            .map_err(|_| TestFailure::RequestBuildFailed)
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
        #[snafu(display("the contest HTTP response body failed"))]
        ResponseBodyFailed,
        #[snafu(display("a contest HTTP response was not valid JSON"))]
        ResponseJsonInvalid,
        #[snafu(display("contest current facts changed at the HTTP boundary"))]
        CurrentFactsChanged,
        #[snafu(display("a redacted contest fact escaped the HTTP boundary"))]
        RedactedFactEscaped,
        #[snafu(display("a contest read changed persisted state"))]
        ReadChangedPersistence,
        #[snafu(display("an empty contest set did not return an empty array"))]
        EmptySetContractChanged,
        #[snafu(display("a contest authentication error response changed"))]
        AuthenticationErrorResponseChanged,
        #[snafu(display("contest authentication failures diverged"))]
        AuthenticationFailuresDiverged,
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
