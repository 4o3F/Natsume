use std::{
    fs,
    path::{Path, PathBuf},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, HeaderName, Method, Request, StatusCode, header},
    middleware as axum_middleware,
    response::IntoResponse as _,
};
use diesel::{QueryableByName, RunQueryDsl, sql_types::BigInt, sqlite::SqliteConnection};
use serde_json::Value;
use snafu::Snafu;
use tower::ServiceExt;
use tracing::instrument::WithSubscriber as _;
use uuid::Uuid;

use crate::{
    audit::CorrelationId,
    component::operator::{
        OperatorCredentials, OperatorRole, hash_password, test_db as db_operator,
        tests::PasswordVerificationTestGuard,
    },
    config::LogLevel,
    db::{
        Database, DatabaseConfig,
        tests::{test_data_version, test_lock_database, test_observer},
    },
    logging::tests::{CapturedLogs, SubscriberTestGuard},
};

use super::{
    error::ApiError,
    handler::health,
    middleware::{CORRELATION_ID_HEADER, correlation_id},
    not_found, router,
};

pub(crate) fn health_router() -> Router {
    Router::new()
        .nest("/api/v2", health::routes())
        .fallback(not_found)
        .layer(axum_middleware::from_fn(correlation_id))
}

pub(super) struct Captured {
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) body: Vec<u8>,
}

pub(super) async fn drive(
    application: &Router,
    request: Request<Body>,
) -> Result<Captured, SupportFailure> {
    let response = application
        .clone()
        .oneshot(request)
        .await
        .map_err(|_| SupportFailure::RouterFailed)?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 16 * 1024)
        .await
        .map_err(|_| SupportFailure::ResponseBodyFailed)?;
    Ok(Captured {
        status,
        headers,
        body: body.to_vec(),
    })
}

pub(super) fn request(
    method: Method,
    uri: &str,
    body: &'static str,
) -> Result<Request<Body>, SupportFailure> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::from(body))
        .map_err(|_| SupportFailure::HelperRequestBuildFailed)
}

pub(super) fn login_request(
    login_name: &str,
    password: &str,
) -> Result<Request<Body>, SupportFailure> {
    let body = serde_json::json!({"login_name": login_name, "password": password}).to_string();
    Request::builder()
        .method(Method::POST)
        .uri("/api/v2/session")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .map_err(|_| SupportFailure::HelperRequestBuildFailed)
}

pub(super) fn cookie_request(
    method: Method,
    uri: &str,
    cookie: &str,
) -> Result<Request<Body>, SupportFailure> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .map_err(|_| SupportFailure::HelperRequestBuildFailed)
}

pub(super) fn header_text<'a>(
    headers: &'a HeaderMap,
    name: &HeaderName,
) -> Result<&'a str, SupportFailure> {
    headers
        .get(name)
        .ok_or(SupportFailure::ResponseHeaderMissing)?
        .to_str()
        .map_err(|_| SupportFailure::ResponseHeaderInvalid)
}

pub(super) fn canonical_correlation_id(headers: &HeaderMap) -> Result<String, SupportFailure> {
    let value = header_text(headers, &CORRELATION_ID_HEADER)?;
    let parsed = Uuid::parse_str(value).map_err(|_| SupportFailure::CorrelationIdInvalid)?;
    if parsed.get_version_num() != 7 || parsed.to_string() != value {
        return Err(SupportFailure::CorrelationIdInvalid);
    }
    Ok(value.to_owned())
}

pub(super) fn check_error_response(
    response: &Captured,
    status: StatusCode,
    title: &str,
    code: &str,
) -> Result<(), SupportFailure> {
    if response.status != status
        || header_text(&response.headers, &header::CONTENT_TYPE)? != "application/json"
    {
        return Err(SupportFailure::ErrorResponseContractChanged);
    }
    let value: Value =
        serde_json::from_slice(&response.body).map_err(|_| SupportFailure::ResponseJsonInvalid)?;
    let object = value
        .as_object()
        .ok_or(SupportFailure::ErrorResponseContractChanged)?;
    let correlation = canonical_correlation_id(&response.headers)?;
    let expected_body = format!(
        "{{\"title\":\"{title}\",\"status\":{},\"code\":\"{code}\",\"correlation_id\":\"{correlation}\"}}",
        status.as_u16(),
    );
    if response.body != expected_body.as_bytes()
        || object.len() != 4
        || object.get("title").and_then(Value::as_str) != Some(title)
        || object.get("status").and_then(Value::as_u64) != Some(u64::from(status.as_u16()))
        || object.get("code").and_then(Value::as_str) != Some(code)
        || object.get("correlation_id").and_then(Value::as_str) != Some(correlation.as_str())
        || object.contains_key("detail")
        || object.contains_key("field_errors")
    {
        return Err(SupportFailure::ErrorResponseContractChanged);
    }
    Ok(())
}

pub(super) fn response_body_text(response: &Captured) -> Result<&str, SupportFailure> {
    std::str::from_utf8(&response.body).map_err(|_| SupportFailure::ResponseBodyFailed)
}

pub(super) fn response_contains(response: &Captured, value: &str) -> bool {
    response_body_text(response).is_ok_and(|body| body.contains(value))
        || response
            .headers
            .values()
            .any(|header| header.to_str().is_ok_and(|header| header.contains(value)))
}

pub(super) struct TestDatabase {
    pub(super) database: Database,
    pub(super) path: PathBuf,
}

impl TestDatabase {
    pub(super) async fn new() -> Result<Self, SupportFailure> {
        let path = std::env::temp_dir().join(format!(
            "natsume-server-http-test-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .map_err(|_| SupportFailure::FixtureFailed)?;
        Ok(Self { database, path })
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _database_result = fs::remove_file(&self.path);
        let _wal_result = fs::remove_file(format!("{}-wal", self.path.display()));
        let _shm_result = fs::remove_file(format!("{}-shm", self.path.display()));
    }
}

struct TestWebRoot {
    path: PathBuf,
}

impl TestWebRoot {
    fn new() -> Result<Self, SupportFailure> {
        let path = std::env::temp_dir().join(format!("natsume-server-web-test-{}", Uuid::now_v7()));
        fs::create_dir(&path).map_err(|_| SupportFailure::FixtureFailed)?;
        let web_root = Self { path };
        fs::create_dir(web_root.path.join("assets")).map_err(|_| SupportFailure::FixtureFailed)?;
        fs::write(web_root.path.join("index.html"), INDEX_HTML)
            .map_err(|_| SupportFailure::FixtureFailed)?;
        fs::write(web_root.path.join("assets/app.js"), APP_JS)
            .map_err(|_| SupportFailure::FixtureFailed)?;
        Ok(web_root)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestWebRoot {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.path);
    }
}

pub(super) async fn seed_operator(
    database: &Database,
    login_name: &str,
    role: OperatorRole,
    password: &str,
) -> Result<Uuid, SupportFailure> {
    let credentials = OperatorCredentials::new(
        login_name.to_owned(),
        password.to_owned(),
        password.to_owned(),
    )
    .map_err(|_| SupportFailure::FixtureFailed)?;
    let password_hash =
        hash_password(credentials.password()).map_err(|_| SupportFailure::FixtureFailed)?;
    db_operator::test_insert_account(database, login_name, role, &password_hash)
        .await
        .map_err(|_| SupportFailure::FixtureFailed)
}

#[derive(Debug, Snafu)]
pub(super) enum SupportFailure {
    #[snafu(display("the HTTP test fixture failed"))]
    FixtureFailed,
    #[snafu(display("the HTTP request could not be built"))]
    HelperRequestBuildFailed,
    #[snafu(display("the HTTP router failed"))]
    RouterFailed,
    #[snafu(display("the HTTP response body failed"))]
    ResponseBodyFailed,
    #[snafu(display("a required HTTP response header was missing"))]
    ResponseHeaderMissing,
    #[snafu(display("an HTTP response header was invalid"))]
    ResponseHeaderInvalid,
    #[snafu(display("the correlation ID contract changed"))]
    CorrelationIdInvalid,
    #[snafu(display("the error response contract changed"))]
    ErrorResponseContractChanged,
    #[snafu(display("an HTTP response was not valid JSON"))]
    ResponseJsonInvalid,
}

const LOGIN_NAME: &str = "http-admin";
const PASSWORD: &str = "http-password-canary";
const LOG_LOGIN_NAME: &str = "structured-log-login-name-canary";
const LOG_PASSWORD: &str = "structured-log-password-canary";
const INDEX_HTML: &str = "<!doctype html><p>packaged-panel-marker</p>";
const APP_JS: &str = "globalThis.natsumePanelMarker = true;\n";
pub(crate) fn unused_web_root() -> &'static Path {
    Path::new("/natsume-server-test-unused-web-root")
}

pub(crate) fn unused_vault_master_key() -> &'static Path {
    Path::new("/natsume-server-test-unused-vault-master-key")
}

#[tokio::test]
async fn packaged_web_panel_and_api_fallbacks_are_isolated() -> Result<(), TestFailure> {
    let web_root = TestWebRoot::new()?;
    let fixture = TestDatabase::new().await?;
    let application = router(
        fixture.database.clone(),
        unused_vault_master_key(),
        web_root.path(),
    );

    for (path, expected_body) in [
        ("/", INDEX_HTML),
        ("/seats", INDEX_HTML),
        ("/assets/app.js", APP_JS),
    ] {
        let response = drive(&application, request(Method::GET, path, "")?).await?;
        if response.status != StatusCode::OK
            || response.body != expected_body.as_bytes()
            || header_text(&response.headers, &header::CACHE_CONTROL)? != "no-cache"
        {
            return Err(TestFailure::StaticPanelContractChanged);
        }
    }

    let api_not_found = drive(
        &application,
        request(Method::GET, "/api/v2/nonexistent", "")?,
    )
    .await?;
    check_error_response(
        &api_not_found,
        StatusCode::NOT_FOUND,
        "Not Found",
        "RESOURCE_NOT_FOUND",
    )?;
    Ok(())
}

#[tokio::test]
async fn mounted_and_unmounted_routes_and_correlation_are_exact() -> Result<(), TestFailure> {
    let closed_fixture = TestDatabase::new().await?;
    let health_router = router(
        closed_fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
    );
    let _database_lock = test_lock_database(&closed_fixture.path)
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let health_response =
        drive(&health_router, request(Method::GET, "/api/v2/health", "")?).await?;
    if health_response.status != StatusCode::OK
        || health_response.body != br#"{"status":"ok"}"#
        || header_text(&health_response.headers, &header::CONTENT_TYPE)? != "application/json"
    {
        return Err(TestFailure::HealthContractChanged);
    }
    let first_correlation = canonical_correlation_id(&health_response.headers)?;

    let fixture = TestDatabase::new().await?;
    let application = router(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
    );
    let supplied = "00000000-0000-7000-8000-000000000000";
    let supplied_request = Request::builder()
        .method(Method::GET)
        .uri("/api/v2/health")
        .header(CORRELATION_ID_HEADER, supplied)
        .body(Body::empty())
        .map_err(|_| TestFailure::RequestBuildFailed)?;
    let supplied_response = drive(&application, supplied_request).await?;
    let second_correlation = canonical_correlation_id(&supplied_response.headers)?;
    if first_correlation == second_correlation || second_correlation == supplied {
        return Err(TestFailure::CorrelationIdWasNotServerOwned);
    }

    let post_request = Request::builder()
        .method(Method::POST)
        .uri("/api/v2/session")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .map_err(|_| TestFailure::RequestBuildFailed)?;
    let mounted = mounted_route_responses(&application, post_request).await?;
    let expected_statuses = [
        StatusCode::BAD_REQUEST,
        StatusCode::UNAUTHORIZED,
        StatusCode::NO_CONTENT,
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
    ];
    for (response, expected_status) in mounted.iter().zip(expected_statuses) {
        if response.status != expected_status {
            return Err(TestFailure::MountedRouteWasNotReachable);
        }
        canonical_correlation_id(&response.headers)?;
    }

    Ok(())
}

async fn mounted_route_responses(
    application: &Router,
    session_post: Request<Body>,
) -> Result<Vec<Captured>, SupportFailure> {
    Ok(vec![
        drive(application, session_post).await?,
        drive(application, request(Method::GET, "/api/v2/session", "")?).await?,
        drive(application, request(Method::DELETE, "/api/v2/session", "")?).await?,
        drive(application, request(Method::GET, "/api/v2/seats", "")?).await?,
        drive(application, request(Method::GET, "/api/v2/accounts", "")?).await?,
        drive(application, request(Method::GET, "/api/v2/bindings", "")?).await?,
        drive(application, request(Method::GET, "/api/v2/imports", "")?).await?,
        drive(
            application,
            request(Method::GET, "/api/v2/provisioning-window", "")?,
        )
        .await?,
        drive(
            application,
            request(Method::POST, "/api/v2/provisioning-window/actions/open", "")?,
        )
        .await?,
        drive(
            application,
            request(
                Method::POST,
                "/api/v2/provisioning-window/actions/close",
                "",
            )?,
        )
        .await?,
        drive(
            application,
            Request::builder()
                .method(Method::POST)
                .uri("/api/v2/imports")
                .header(header::CONTENT_TYPE, "text/csv")
                .body(Body::empty())
                .map_err(|_| SupportFailure::HelperRequestBuildFailed)?,
        )
        .await?,
        drive(
            application,
            request(
                Method::POST,
                "/api/v2/imports/01900000-0000-7000-8000-000000000000/actions/commit",
                "",
            )?,
        )
        .await?,
        drive(
            application,
            request(
                Method::POST,
                "/api/v2/imports/01900000-0000-7000-8000-000000000000/actions/discard",
                "",
            )?,
        )
        .await?,
    ])
}

#[tokio::test]
async fn completed_request_log_is_single_bounded_and_correlated() -> Result<(), TestFailure> {
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let fixture = TestDatabase::new().await?;
    let application = router(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
    );
    let captured = CapturedLogs::default();
    let subscriber = captured.subscriber(LogLevel::Info);
    let response = async { drive(&application, request(Method::GET, "/api/v2/health", "")?).await }
        .with_subscriber(subscriber)
        .await?;
    let correlation_id = canonical_correlation_id(&response.headers)?;
    let output = captured
        .text()
        .map_err(|()| TestFailure::LogCaptureFailed)?;
    if output.matches("HTTP request completed").count() != 1
        || !output.contains("method=GET")
        || !output.contains("path=/api/v2/health")
        || !output.contains("status=200")
        || !output.contains(&format!("correlation_id={correlation_id}"))
        || !output.contains("duration_us=")
    {
        return Err(TestFailure::CompletedRequestLogChanged);
    }
    Ok(())
}

#[tokio::test]
async fn login_and_error_logs_enforce_the_redaction_contract() -> Result<(), TestFailure> {
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(
        &fixture.database,
        LOG_LOGIN_NAME,
        OperatorRole::Admin,
        LOG_PASSWORD,
    )
    .await?;
    let application = router(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
    );
    let captured = CapturedLogs::default();
    let subscriber = captured.subscriber(LogLevel::Trace);
    let (credential, authentication_correlation, internal_correlation) = async {
        let login = drive(&application, login_request(LOG_LOGIN_NAME, LOG_PASSWORD)?).await?;
        if login.status != StatusCode::OK {
            return Err(TestFailure::ValidLoginFailed);
        }
        let cookie_pair = header_text(&login.headers, &header::SET_COOKIE)?
            .split(';')
            .next()
            .ok_or(TestFailure::CookieContractChanged)?
            .to_owned();
        let credential = cookie_pair
            .split_once('=')
            .map(|(_, value)| value.to_owned())
            .ok_or(TestFailure::CookieContractChanged)?;
        let authenticated = drive(
            &application,
            cookie_request(Method::GET, "/api/v2/session", &cookie_pair)?,
        )
        .await?;
        if authenticated.status != StatusCode::OK {
            return Err(TestFailure::ValidLoginFailed);
        }
        let authentication_failure =
            drive(&application, request(Method::GET, "/api/v2/session", "")?).await?;
        let authentication_correlation = canonical_correlation_id(&authentication_failure.headers)?;
        let internal_correlation = CorrelationId::from_uuid(Uuid::now_v7());
        let _internal_response =
            ApiError::internal_error("test_internal_cause_canary", internal_correlation)
                .into_response();
        Ok((
            credential,
            authentication_correlation,
            internal_correlation.as_text(),
        ))
    }
    .with_subscriber(subscriber)
    .await?;
    let output = captured
        .text()
        .map_err(|()| TestFailure::LogCaptureFailed)?;
    let uppercase_output = output.to_ascii_uppercase();
    let database_path = fixture.path.to_string_lossy();
    for forbidden in [
        LOG_PASSWORD,
        LOG_LOGIN_NAME,
        credential.as_str(),
        database_path.as_ref(),
        "$argon2id$",
        "__Secure-natsume_session",
        "operator session authentication failed",
        "source chain",
    ] {
        if output.contains(forbidden) {
            return Err(TestFailure::SecretEscapedIntoLog);
        }
    }
    if uppercase_output.contains("SELECT")
        || uppercase_output.contains("INSERT")
        || !output.contains("WARN HTTP request rejected")
        || !output.contains("code=\"AUTHENTICATION_FAILED\"")
        || !output.contains(&format!("correlation_id={authentication_correlation}"))
        || !output.contains("ERROR HTTP request failed")
        || !output.contains("code=\"INTERNAL_ERROR\"")
        || !output.contains(&format!("correlation_id={internal_correlation}"))
    {
        return Err(TestFailure::ErrorLogChanged);
    }
    Ok(())
}

/// The expired-cleanup warning is the only place an operator store cause
/// survives, and it is emitted inside a `Database::read` closure, so this
/// also proves the blocking thread reaches the scoped subscriber. A write-locked
/// database blocks the lazy-expiry escalation, and the pool's `busy_timeout` is
/// 5000 ms, so the request below spends about five seconds blocked.
#[tokio::test]
async fn blocked_expiry_cleanup_logs_its_cause_and_never_returns_it() -> Result<(), TestFailure> {
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(&fixture.database, LOGIN_NAME, OperatorRole::Admin, PASSWORD).await?;
    let application = router(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
    );
    let login_response = drive(&application, login_request(LOGIN_NAME, PASSWORD)?).await?;
    if login_response.status != StatusCode::OK {
        return Err(TestFailure::ValidLoginFailed);
    }
    let cookie_pair = header_text(&login_response.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or(TestFailure::CookieContractChanged)?
        .to_owned();
    db_operator::test_expire_all_sessions(&fixture.database)
        .await
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let _database_lock =
        test_lock_database(&fixture.path).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;

    let captured = CapturedLogs::default();
    let subscriber = captured.subscriber(LogLevel::Warn);
    let response = async {
        drive(
            &application,
            cookie_request(Method::GET, "/api/v2/session", &cookie_pair)?,
        )
        .await
    }
    .with_subscriber(subscriber)
    .await?;

    let output = captured
        .text()
        .map_err(|()| TestFailure::LogCaptureFailed)?;
    if !output.contains("expired operator session cleanup failed")
        || !output.contains("cause=\"operator_store_transaction_failed\"")
    {
        return Err(TestFailure::ExpiredCleanupCauseWasNotLogged);
    }
    if response.status != StatusCode::UNAUTHORIZED
        || response_contains(&response, "operator_store_transaction_failed")
        || response_contains(&response, "operator_store")
    {
        return Err(TestFailure::ExpiredCleanupCauseEscapedIntoResponse);
    }
    Ok(())
}

#[tokio::test]
async fn head_is_rejected_without_session_persistence_access() -> Result<(), TestFailure> {
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(&fixture.database, LOGIN_NAME, OperatorRole::Admin, PASSWORD).await?;
    let application = router(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
    );
    let login_response = drive(&application, login_request(LOGIN_NAME, PASSWORD)?).await?;
    if login_response.status != StatusCode::OK {
        return Err(TestFailure::ValidLoginFailed);
    }
    let cookie_pair = header_text(&login_response.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or(TestFailure::CookieContractChanged)?
        .to_owned();
    let mut observer =
        test_observer(&fixture.path).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let counts_before = session_and_audit_counts_on(&mut observer)?;
    let version_before =
        test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let _database_lock =
        test_lock_database(&fixture.path).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;

    let health_head = drive(&application, request(Method::HEAD, "/api/v2/health", "")?).await?;
    let session_head = drive(
        &application,
        cookie_request(Method::HEAD, "/api/v2/session", &cookie_pair)?,
    )
    .await?;
    let session_head_without_cookie =
        drive(&application, request(Method::HEAD, "/api/v2/session", "")?).await?;
    let session_head_with_malformed_cookie = drive(
        &application,
        cookie_request(
            Method::HEAD,
            "/api/v2/session",
            "__Secure-natsume_session=not-a-session-credential",
        )?,
    )
    .await?;
    for response in [
        &health_head,
        &session_head,
        &session_head_without_cookie,
        &session_head_with_malformed_cookie,
    ] {
        if response.status != StatusCode::NOT_FOUND
            || header_text(&response.headers, &header::CONTENT_TYPE)? != "application/json"
        {
            return Err(TestFailure::HeadRouteWasReachable);
        }
        canonical_correlation_id(&response.headers)?;
    }
    if session_and_audit_counts_on(&mut observer)? != counts_before
        || test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?
            != version_before
    {
        return Err(TestFailure::HeadRequestAccessedSessionPersistence);
    }
    Ok(())
}

/// `authenticate_operator` short-circuits `HEAD` before authenticating, so every
/// protected route must answer `HEAD` without reaching a real handler. The
/// GET routes are built by `middleware::operator_get`, and the action routes are
/// `post`-only, so `HEAD` is rejected by method routing before the auth layer.
#[tokio::test]
async fn head_on_every_protected_route_never_reaches_a_handler() -> Result<(), TestFailure> {
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(&fixture.database, LOGIN_NAME, OperatorRole::Admin, PASSWORD).await?;
    let application = router(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
    );
    let login_response = drive(&application, login_request(LOGIN_NAME, PASSWORD)?).await?;
    if login_response.status != StatusCode::OK {
        return Err(TestFailure::ValidLoginFailed);
    }
    let cookie_pair = header_text(&login_response.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or(TestFailure::CookieContractChanged)?
        .to_owned();

    for (path, expected_status) in [
        ("/api/v2/session", StatusCode::NOT_FOUND),
        ("/api/v2/seats", StatusCode::NOT_FOUND),
        ("/api/v2/accounts", StatusCode::NOT_FOUND),
        ("/api/v2/bindings", StatusCode::NOT_FOUND),
        ("/api/v2/imports", StatusCode::NOT_FOUND),
        ("/api/v2/provisioning-window", StatusCode::NOT_FOUND),
        (
            "/api/v2/provisioning-window/actions/open",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            "/api/v2/provisioning-window/actions/close",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
    ] {
        for cookie in [None, Some(cookie_pair.as_str())] {
            let head = match cookie {
                Some(cookie) => cookie_request(Method::HEAD, path, cookie)?,
                None => request(Method::HEAD, path, "")?,
            };
            let response = drive(&application, head).await?;
            if response.status != expected_status {
                return Err(TestFailure::HeadRouteWasReachable);
            }
            canonical_correlation_id(&response.headers)?;
            // A reached handler would answer 200; `HEAD` bodies are stripped, so
            // the status and the uniform JSON error shape are the evidence.
            if expected_status == StatusCode::NOT_FOUND
                && (header_text(&response.headers, &header::CONTENT_TYPE)? != "application/json"
                    || !response.body.is_empty())
            {
                return Err(TestFailure::HeadRouteWasReachable);
            }
        }
    }
    Ok(())
}

fn session_and_audit_counts_on(
    connection: &mut SqliteConnection,
) -> Result<(i64, i64), TestFailure> {
    diesel::sql_query(
        "SELECT (SELECT COUNT(*) FROM operator_sessions) AS sessions, \
         (SELECT COUNT(*) FROM audit_events) AS audits",
    )
    .get_result::<PersistenceCountsRow>(connection)
    .map(|row| (row.sessions, row.audits))
    .map_err(|_| TestFailure::DatabaseEvidenceFailed)
}

#[derive(QueryableByName)]
struct PersistenceCountsRow {
    #[diesel(sql_type = BigInt)]
    sessions: i64,
    #[diesel(sql_type = BigInt)]
    audits: i64,
}

#[derive(Debug, Snafu)]
enum TestFailure {
    #[snafu(display("an HTTP test helper failed"))]
    #[snafu(context(false))]
    Support { source: SupportFailure },
    #[snafu(display("captured HTTP logs could not be read"))]
    LogCaptureFailed,
    #[snafu(display("the completed-request log contract changed"))]
    CompletedRequestLogChanged,
    #[snafu(display("a secret or forbidden diagnostic escaped into logs"))]
    SecretEscapedIntoLog,
    #[snafu(display("the stable HTTP error log contract changed"))]
    ErrorLogChanged,
    #[snafu(display("the HTTP request could not be built"))]
    RequestBuildFailed,
    #[snafu(display("the health contract changed"))]
    HealthContractChanged,
    #[snafu(display("the static Panel serving contract changed"))]
    StaticPanelContractChanged,
    #[snafu(display("a client correlation ID was accepted"))]
    CorrelationIdWasNotServerOwned,
    #[snafu(display("a mounted HTTP route was unreachable"))]
    MountedRouteWasNotReachable,
    #[snafu(display("an undeclared HEAD route was reachable"))]
    HeadRouteWasReachable,
    #[snafu(display("a rejected HEAD request accessed session persistence"))]
    HeadRequestAccessedSessionPersistence,
    #[snafu(display("a valid operator login failed"))]
    ValidLoginFailed,
    #[snafu(display("the session cookie contract changed"))]
    CookieContractChanged,
    #[snafu(display("database evidence could not be read"))]
    DatabaseEvidenceFailed,
    #[snafu(display("the blocked expired-cleanup cause was not logged"))]
    ExpiredCleanupCauseWasNotLogged,
    #[snafu(display("the expired-cleanup cause escaped into an HTTP response"))]
    ExpiredCleanupCauseEscapedIntoResponse,
}
