use axum::{
    Extension, Json, Router,
    extract::{Request, State},
    middleware as axum_middleware,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    application::{
        operator::{self, OperatorIdentity},
        provisioning::{self, ProvisioningWindow, ProvisioningWindowState},
    },
    audit::CorrelationId,
};

use super::super::{AppState, error::ApiError, middleware};

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    let open =
        post(open_provisioning_window).route_layer(axum_middleware::from_fn(require_admin_role));
    let close =
        post(close_provisioning_window).route_layer(axum_middleware::from_fn(require_admin_role));
    Router::new()
        .route(
            "/provisioning-window",
            middleware::operator_get(state.clone(), get_provisioning_window),
        )
        .route(
            "/provisioning-window/actions/open",
            middleware::require_operator(state.clone(), open),
        )
        .route(
            "/provisioning-window/actions/close",
            middleware::require_operator(state, close),
        )
}

async fn require_admin_role(
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(identity): Extension<OperatorIdentity>,
    request: Request,
    next: Next,
) -> Response {
    if let Err(error) = operator::require_admin(identity.role()) {
        return ApiError::from_operator(error, correlation_id).into_response();
    }
    next.run(request).await
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvisioningWindowResponse {
    #[schema(inline)]
    state: ProvisioningWindowResponseState,
    revision: i64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum ProvisioningWindowResponseState {
    Closed,
    Open,
}

impl From<ProvisioningWindowState> for ProvisioningWindowResponseState {
    fn from(state: ProvisioningWindowState) -> Self {
        match state {
            ProvisioningWindowState::Closed => Self::Closed,
            ProvisioningWindowState::Open => Self::Open,
        }
    }
}

impl From<ProvisioningWindow> for ProvisioningWindowResponse {
    fn from(window: ProvisioningWindow) -> Self {
        Self {
            state: ProvisioningWindowResponseState::from(window.state),
            revision: window.revision,
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/provisioning-window",
    operation_id = "getProvisioningWindow",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current provisioning-window fact", body = ProvisioningWindowResponse),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn get_provisioning_window(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
) -> Response {
    match provisioning::read_window(&state.database).await {
        Ok(window) => Json(ProvisioningWindowResponse::from(window)).into_response(),
        Err(error) => ApiError::from_provisioning(error, correlation_id).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/provisioning-window/actions/open",
    operation_id = "openProvisioningWindow",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Provisioning window opened or already open", body = ProvisioningWindowResponse),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn open_provisioning_window(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
) -> Response {
    match provisioning::open_window(&state.database, correlation_id).await {
        Ok(window) => Json(ProvisioningWindowResponse::from(window)).into_response(),
        Err(error) => ApiError::from_provisioning(error, correlation_id).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/provisioning-window/actions/close",
    operation_id = "closeProvisioningWindow",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Provisioning window closed or already closed", body = ProvisioningWindowResponse),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn close_provisioning_window(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
) -> Response {
    match provisioning::close_window(&state.database, correlation_id).await {
        Ok(window) => Json(ProvisioningWindowResponse::from(window)).into_response(),
        Err(error) => ApiError::from_provisioning(error, correlation_id).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Method, Request, StatusCode, header},
    };
    use diesel::{
        Connection, QueryableByName, RunQueryDsl,
        connection::SimpleConnection,
        sql_types::{BigInt, Nullable, Text},
        sqlite::SqliteConnection,
    };
    use serde_json::Value;
    use snafu::Snafu;
    use uuid::Uuid;

    use crate::{
        application::operator::{OperatorRole, sign_in, tests::PasswordVerificationTestGuard},
        audit::CorrelationId,
        db::Database,
    };

    use super::super::super::{
        router,
        tests::{
            Captured, SupportFailure, TestDatabase, canonical_correlation_id, check_error_response,
            drive, header_text, seed_operator, unused_vault_master_key, unused_web_root,
        },
    };

    const ADMIN_LOGIN: &str = "provisioning-http-admin";
    const VIEWER_LOGIN: &str = "provisioning-http-viewer";
    const PASSWORD: &str = "provisioning-http-password-canary";
    const READ_PATH: &str = "/api/v2/provisioning-window";
    const OPEN_PATH: &str = "/api/v2/provisioning-window/actions/open";
    const CLOSE_PATH: &str = "/api/v2/provisioning-window/actions/close";

    #[tokio::test]
    async fn provisioning_window_role_matrix_and_current_fact_read_are_exact()
    -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        seed_operator(
            &fixture.database,
            ADMIN_LOGIN,
            OperatorRole::Admin,
            PASSWORD,
        )
        .await?;
        seed_operator(
            &fixture.database,
            VIEWER_LOGIN,
            OperatorRole::Viewer,
            PASSWORD,
        )
        .await?;
        let _verification_guard = PasswordVerificationTestGuard::acquire().await;
        let admin_cookie = session_cookie(&fixture.database, ADMIN_LOGIN).await?;
        let viewer_cookie = session_cookie(&fixture.database, VIEWER_LOGIN).await?;
        let application = test_router(&fixture);

        for (method, path) in [
            (Method::GET, READ_PATH),
            (Method::POST, OPEN_PATH),
            (Method::POST, CLOSE_PATH),
        ] {
            let response = drive(&application, request(method, path, None)?).await?;
            check_error_response(
                &response,
                StatusCode::UNAUTHORIZED,
                "Unauthorized",
                "AUTHENTICATION_FAILED",
            )?;
        }
        for path in [OPEN_PATH, CLOSE_PATH] {
            let response = drive(
                &application,
                request(Method::POST, path, Some(&viewer_cookie))?,
            )
            .await?;
            check_error_response(
                &response,
                StatusCode::FORBIDDEN,
                "Forbidden",
                "AUTHORIZATION_DENIED",
            )?;
        }

        let viewer_read = drive(
            &application,
            request(Method::GET, READ_PATH, Some(&viewer_cookie))?,
        )
        .await?;
        let admin_read = drive(
            &application,
            request(Method::GET, READ_PATH, Some(&admin_cookie))?,
        )
        .await?;
        assert_window_response(&viewer_read, "closed", 0)?;
        assert_window_response(&admin_read, "closed", 0)?;
        if viewer_read.body != admin_read.body
            || !operator_window_audits(&fixture.database).await?.is_empty()
        {
            return Err(TestFailure::RoleMatrixChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn open_close_open_and_repeat_noops_return_current_facts_and_exact_audits()
    -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        seed_operator(
            &fixture.database,
            ADMIN_LOGIN,
            OperatorRole::Admin,
            PASSWORD,
        )
        .await?;
        let _verification_guard = PasswordVerificationTestGuard::acquire().await;
        let admin_cookie = session_cookie(&fixture.database, ADMIN_LOGIN).await?;
        let application = test_router(&fixture);
        let operations = [
            (
                OPEN_PATH,
                "open",
                1,
                "open_provisioning_window",
                "succeeded",
                "operator_requested",
                0,
                1,
            ),
            (
                OPEN_PATH,
                "open",
                1,
                "open_provisioning_window",
                "noop",
                "target_already_satisfied",
                1,
                1,
            ),
            (
                CLOSE_PATH,
                "closed",
                2,
                "close_provisioning_window",
                "succeeded",
                "operator_requested",
                1,
                2,
            ),
            (
                CLOSE_PATH,
                "closed",
                2,
                "close_provisioning_window",
                "noop",
                "target_already_satisfied",
                2,
                2,
            ),
            (
                OPEN_PATH,
                "open",
                3,
                "open_provisioning_window",
                "succeeded",
                "operator_requested",
                2,
                3,
            ),
        ];
        let mut correlations = Vec::new();
        for operation in operations {
            let response = drive(
                &application,
                request(Method::POST, operation.0, Some(&admin_cookie))?,
            )
            .await?;
            correlations.push(assert_window_response(&response, operation.1, operation.2)?);
        }

        let audits = operator_window_audits(&fixture.database).await?;
        if audits.len() != operations.len() {
            return Err(TestFailure::WindowAuditChanged);
        }
        for (index, operation) in operations.into_iter().enumerate() {
            assert_audit(
                &audits[index],
                operation.3,
                operation.4,
                operation.5,
                operation.6,
                operation.7,
                &correlations[index],
            )?;
        }
        let window = window_evidence(&fixture.database).await?;
        if window.state != "open"
            || window.revision != 3
            || window.last_audit_event_id.as_deref()
                != audits.last().map(|audit| audit.audit_event_id.as_str())
        {
            return Err(TestFailure::WindowCycleChanged);
        }
        assert_read_window(&application, &admin_cookie, "open", 3).await?;
        Ok(())
    }

    #[tokio::test]
    async fn provisioning_failures_map_only_to_explicit_internal_errors() -> Result<(), TestFailure>
    {
        let fixture = TestDatabase::new().await?;
        seed_operator(
            &fixture.database,
            ADMIN_LOGIN,
            OperatorRole::Admin,
            PASSWORD,
        )
        .await?;
        let _verification_guard = PasswordVerificationTestGuard::acquire().await;
        let admin_cookie = session_cookie(&fixture.database, ADMIN_LOGIN).await?;
        let application = test_router(&fixture);
        let opened = drive(
            &application,
            request(Method::POST, OPEN_PATH, Some(&admin_cookie))?,
        )
        .await?;
        assert_window_response(&opened, "open", 1)?;
        set_window_revision(&fixture.database, i64::MAX).await?;

        let overflow = drive(
            &application,
            request(Method::POST, CLOSE_PATH, Some(&admin_cookie))?,
        )
        .await?;
        check_error_response(
            &overflow,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            "INTERNAL_ERROR",
        )?;
        let after_overflow = window_evidence(&fixture.database).await?;
        if after_overflow.state != "open" || after_overflow.revision != i64::MAX {
            return Err(TestFailure::FailedMutationChangedWindow);
        }

        poison_window_state(&fixture.path)?;
        let invalid_facts = drive(
            &application,
            request(Method::GET, READ_PATH, Some(&admin_cookie))?,
        )
        .await?;
        check_error_response(
            &invalid_facts,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            "INTERNAL_ERROR",
        )?;
        Ok(())
    }

    fn test_router(fixture: &TestDatabase) -> Router {
        router(
            fixture.database.clone(),
            unused_vault_master_key(),
            unused_web_root(),
        )
    }

    async fn assert_read_window(
        application: &Router,
        cookie: &str,
        state: &str,
        revision: i64,
    ) -> Result<(), TestFailure> {
        let read = drive(application, request(Method::GET, READ_PATH, Some(cookie))?).await?;
        assert_window_response(&read, state, revision)?;
        Ok(())
    }

    async fn session_cookie(database: &Database, login_name: &str) -> Result<String, TestFailure> {
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

    fn request(
        method: Method,
        path: &str,
        cookie: Option<&str>,
    ) -> Result<Request<Body>, TestFailure> {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        request
            .body(Body::empty())
            .map_err(|_| TestFailure::RequestBuildFailed)
    }

    fn assert_window_response(
        response: &Captured,
        expected_state: &str,
        expected_revision: i64,
    ) -> Result<String, TestFailure> {
        if response.status != StatusCode::OK
            || header_text(&response.headers, &header::CONTENT_TYPE)? != "application/json"
        {
            return Err(TestFailure::WindowResponseChanged);
        }
        let correlation_id = canonical_correlation_id(&response.headers)?;
        let value: Value = serde_json::from_slice(&response.body)
            .map_err(|_| TestFailure::WindowResponseChanged)?;
        let object = value
            .as_object()
            .ok_or(TestFailure::WindowResponseChanged)?;
        let expected = format!(r#"{{"state":"{expected_state}","revision":{expected_revision}}}"#);
        let expected_revision =
            u64::try_from(expected_revision).map_err(|_| TestFailure::WindowResponseChanged)?;
        if response.body != expected.as_bytes()
            || object.len() != 2
            || object.get("state").and_then(Value::as_str) != Some(expected_state)
            || object.get("revision").and_then(Value::as_u64) != Some(expected_revision)
        {
            return Err(TestFailure::WindowResponseChanged);
        }
        Ok(correlation_id)
    }

    async fn operator_window_audits(
        database: &Database,
    ) -> Result<Vec<OperatorWindowAudit>, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT audit_event_id, actor, action_kind, resource_type, resource_id, \
                     result, reason_code, correlation_id, group_correlation_id, \
                     redacted_detail_json FROM audit_events \
                     WHERE actor = 'operator:self' AND resource_type = 'provisioning_window' \
                     ORDER BY rowid",
                )
                .load::<OperatorWindowAudit>(connection)
                .map_err(|_| TestFailure::DatabaseEvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
    }

    async fn window_evidence(database: &Database) -> Result<WindowEvidence, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT state, revision, last_audit_event_id \
                     FROM provisioning_window WHERE singleton = 1",
                )
                .get_result::<WindowEvidence>(connection)
                .map_err(|_| TestFailure::DatabaseEvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
    }

    async fn set_window_revision(database: &Database, revision: i64) -> Result<(), TestFailure> {
        database
            .interact(move |connection| {
                diesel::sql_query("UPDATE provisioning_window SET revision = ? WHERE singleton = 1")
                    .bind::<BigInt, _>(revision)
                    .execute(connection)
                    .map(|_| ())
                    .map_err(|_| TestFailure::FixtureFailed)
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
    }

    fn poison_window_state(path: &std::path::Path) -> Result<(), TestFailure> {
        let path = path.to_str().ok_or(TestFailure::FixtureFailed)?;
        let mut connection =
            SqliteConnection::establish(path).map_err(|_| TestFailure::FixtureFailed)?;
        connection
            .batch_execute(
                "PRAGMA ignore_check_constraints = ON; \
                 UPDATE provisioning_window SET state = 'invalid-test-state' WHERE singleton = 1;",
            )
            .map_err(|_| TestFailure::FixtureFailed)
    }

    #[allow(clippy::too_many_arguments)]
    fn assert_audit(
        audit: &OperatorWindowAudit,
        action_kind: &str,
        result: &str,
        reason_code: &str,
        previous_revision: i64,
        new_revision: i64,
        correlation_id: &str,
    ) -> Result<(), TestFailure> {
        let detail =
            format!(r#"{{"previous_revision":{previous_revision},"new_revision":{new_revision}}}"#);
        if audit.actor != "operator:self"
            || audit.action_kind != action_kind
            || audit.resource_type != "provisioning_window"
            || audit.resource_id.is_some()
            || audit.result != result
            || audit.reason_code.as_deref() != Some(reason_code)
            || audit.correlation_id != correlation_id
            || audit.group_correlation_id.is_some()
            || audit.redacted_detail_json != detail
        {
            return Err(TestFailure::WindowAuditChanged);
        }
        Ok(())
    }

    #[derive(QueryableByName)]
    struct WindowEvidence {
        #[diesel(sql_type = Text)]
        state: String,
        #[diesel(sql_type = BigInt)]
        revision: i64,
        #[diesel(sql_type = Nullable<Text>)]
        last_audit_event_id: Option<String>,
    }

    #[derive(QueryableByName)]
    struct OperatorWindowAudit {
        #[diesel(sql_type = Text)]
        audit_event_id: String,
        #[diesel(sql_type = Text)]
        actor: String,
        #[diesel(sql_type = Text)]
        action_kind: String,
        #[diesel(sql_type = Text)]
        resource_type: String,
        #[diesel(sql_type = Nullable<Text>)]
        resource_id: Option<String>,
        #[diesel(sql_type = Text)]
        result: String,
        #[diesel(sql_type = Nullable<Text>)]
        reason_code: Option<String>,
        #[diesel(sql_type = Text)]
        correlation_id: String,
        #[diesel(sql_type = Nullable<Text>)]
        group_correlation_id: Option<String>,
        #[diesel(sql_type = Text)]
        redacted_detail_json: String,
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("an HTTP test helper failed"))]
        #[snafu(context(false))]
        Support { source: SupportFailure },
        #[snafu(display("the provisioning HTTP fixture failed"))]
        FixtureFailed,
        #[snafu(display("a provisioning HTTP request could not be built"))]
        RequestBuildFailed,
        #[snafu(display("the provisioning role matrix changed"))]
        RoleMatrixChanged,
        #[snafu(display("the provisioning-window response changed"))]
        WindowResponseChanged,
        #[snafu(display("the provisioning-window cycle changed"))]
        WindowCycleChanged,
        #[snafu(display("the provisioning-window audit changed"))]
        WindowAuditChanged,
        #[snafu(display("provisioning-window database evidence could not be read"))]
        DatabaseEvidenceFailed,
        #[snafu(display("a failed provisioning mutation changed the window"))]
        FailedMutationChangedWindow,
    }
}
