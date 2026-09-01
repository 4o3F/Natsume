use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, HeaderName, Method, Request, StatusCode, header},
    middleware as axum_middleware,
    response::IntoResponse as _,
};
use diesel::{QueryableByName, RunQueryDsl, sql_types::BigInt, sqlite::SqliteConnection};
use opentelemetry::trace::{SpanKind, TracerProvider as _};
use opentelemetry_sdk::{
    error::OTelSdkResult,
    trace::{SdkTracerProvider, SpanData, SpanExporter},
};
use serde_json::Value;
use snafu::Snafu;
use tower::ServiceExt;
use tracing::{
    Subscriber,
    field::{Field, Visit},
    instrument::WithSubscriber as _,
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt as _},
    registry::{LookupSpan, Registry},
};
use uuid::Uuid;

use crate::{
    component::operator::{
        OperatorCredentials,
        tests::{self as db_operator, PasswordVerificationTestGuard},
    },
    config::LogLevel,
    db::{
        Database, DatabaseConfig,
        tests::{test_data_version, test_lock_database, test_observer},
    },
    logging::tests::{CapturedLogs, SubscriberTestGuard},
    server_state::ServerState,
    vault,
};

use super::{
    AppState, error::ApiError, handler::health, middleware::request_context, not_found, router,
};

pub(crate) fn health_router() -> Router {
    Router::new()
        .nest("/api/v2", health::routes())
        .fallback(not_found)
        .layer(axum_middleware::from_fn(request_context))
}

pub(super) struct Captured {
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) body: Vec<u8>,
}

#[derive(Clone, Default)]
struct CapturedRequestContext {
    fields: Arc<Mutex<RequestContextFields>>,
}

impl CapturedRequestContext {
    fn update(&self, update: impl FnOnce(&mut RequestContextFields)) {
        let mut fields = self.fields.lock().unwrap_or_else(PoisonError::into_inner);
        update(&mut fields);
    }

    fn snapshot(&self) -> RequestContextFields {
        self.fields
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl<S> Layer<S> for CapturedRequestContext
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attributes: &Attributes<'_>, _id: &Id, _context: Context<'_, S>) {
        if attributes.metadata().name() != "http_request" {
            return;
        }
        self.update(|fields| {
            fields.span_count += 1;
            fields.span_name = Some(attributes.metadata().name());
            attributes.record(&mut RequestContextVisitor { fields });
        });
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, context: Context<'_, S>) {
        let Some(span) = context.span(id) else {
            return;
        };
        if span.metadata().name() != "http_request" {
            return;
        }
        self.update(|fields| values.record(&mut RequestContextVisitor { fields }));
    }
}

#[derive(Clone, Default)]
struct RequestContextFields {
    span_count: usize,
    span_name: Option<&'static str>,
    http_method: Option<String>,
    http_route: Option<String>,
    http_status: Option<u64>,
    outcome: Option<String>,
    otel_status: Option<String>,
    actor_kind: Option<String>,
    actor_id: Option<String>,
}

struct RequestContextVisitor<'fields> {
    fields: &'fields mut RequestContextFields,
}

impl Visit for RequestContextVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        let destination = match field.name() {
            "http.request.method" => &mut self.fields.http_method,
            "http.route" => &mut self.fields.http_route,
            "request.outcome" => &mut self.fields.outcome,
            "otel.status_code" => &mut self.fields.otel_status,
            "actor_kind" => &mut self.fields.actor_kind,
            "actor_id" => &mut self.fields.actor_id,
            _ => return,
        };
        *destination = Some(value.to_owned());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "http.response.status_code" {
            self.fields.http_status = Some(value);
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
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
    let expected_body = format!(
        "{{\"title\":\"{title}\",\"status\":{},\"code\":\"{code}\"}}",
        status.as_u16(),
    );
    if response.body != expected_body.as_bytes()
        || response.headers.contains_key("x-correlation-id")
        || object.len() != 3
        || object.get("title").and_then(Value::as_str) != Some(title)
        || object.get("status").and_then(Value::as_u64) != Some(u64::from(status.as_u16()))
        || object.get("code").and_then(Value::as_str) != Some(code)
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
    password: &str,
) -> Result<Uuid, SupportFailure> {
    let credentials = OperatorCredentials::new(
        login_name.to_owned(),
        password.to_owned(),
        password.to_owned(),
    )
    .map_err(|_| SupportFailure::FixtureFailed)?;
    let password_hash = credentials
        .hash_password()
        .map_err(|_| SupportFailure::FixtureFailed)?;
    db_operator::test_insert_admin_account(database, login_name, &password_hash)
        .await
        .map_err(|_| SupportFailure::FixtureFailed)
}

#[derive(Debug, Snafu)]
pub(crate) enum SupportFailure {
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

pub(crate) fn server_state(database: Database) -> Result<AppState, SupportFailure> {
    let root = std::env::temp_dir().join(format!("natsume-server-state-{}", Uuid::now_v7()));
    fs::create_dir(&root).map_err(|_| SupportFailure::FixtureFailed)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
        .map_err(|_| SupportFailure::FixtureFailed)?;
    let key_path = root.join("master.key");
    vault::ensure_master_key(&key_path).map_err(|_| SupportFailure::FixtureFailed)?;
    let vault = vault::load(&key_path).map_err(|_| SupportFailure::FixtureFailed)?;
    fs::remove_dir_all(root).map_err(|_| SupportFailure::FixtureFailed)?;
    Ok(Arc::new(ServerState::new(database, Arc::new(vault))))
}

#[tokio::test]
async fn packaged_web_panel_and_api_fallbacks_are_isolated() -> Result<(), TestFailure> {
    let web_root = TestWebRoot::new()?;
    let fixture = TestDatabase::new().await?;
    let application = router(server_state(fixture.database.clone())?, web_root.path());

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
async fn mounted_and_unmounted_routes_are_exact_without_correlation_contract()
-> Result<(), TestFailure> {
    let closed_fixture = TestDatabase::new().await?;
    let health_router = router(
        server_state(closed_fixture.database.clone())?,
        unused_web_root(),
    );
    let _database_lock = test_lock_database(&closed_fixture.path)
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let health_response =
        drive(&health_router, request(Method::GET, "/api/v2/health", "")?).await?;
    if health_response.status != StatusCode::OK
        || health_response.body != br#"{"status":"ok"}"#
        || header_text(&health_response.headers, &header::CONTENT_TYPE)? != "application/json"
        || health_response.headers.contains_key("x-correlation-id")
    {
        return Err(TestFailure::HealthContractChanged);
    }

    let fixture = TestDatabase::new().await?;
    let application = router(server_state(fixture.database.clone())?, unused_web_root());
    let supplied = "00000000-0000-7000-8000-000000000000";
    let supplied_request = Request::builder()
        .method(Method::GET)
        .uri("/api/v2/health")
        .header("x-correlation-id", supplied)
        .body(Body::empty())
        .map_err(|_| TestFailure::RequestBuildFailed)?;
    let supplied_response = drive(&application, supplied_request).await?;
    if supplied_response.headers.contains_key("x-correlation-id")
        || response_contains(&supplied_response, supplied)
    {
        return Err(TestFailure::CorrelationContractWasRetained);
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
        if response.headers.contains_key("x-correlation-id") {
            return Err(TestFailure::CorrelationContractWasRetained);
        }
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
async fn completed_request_log_is_single_and_uses_the_route_template() -> Result<(), TestFailure> {
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let fixture = TestDatabase::new().await?;
    let application = router(server_state(fixture.database.clone())?, unused_web_root());
    let captured = CapturedLogs::default();
    let subscriber = captured.subscriber(LogLevel::Info);
    let response = async { drive(&application, request(Method::GET, "/api/v2/health", "")?).await }
        .with_subscriber(subscriber)
        .await?;
    let output = captured
        .text()
        .map_err(|()| TestFailure::LogCaptureFailed)?;
    if output.matches("HTTP request completed").count() != 1
        || response.status != StatusCode::OK
        || !output.contains("method=GET")
        || !output.contains("route=\"/api/v2/health\"")
        || !output.contains("status=200")
        || !output.contains("duration_us=")
        || output.contains("correlation_id")
        || response.headers.contains_key("x-correlation-id")
    {
        return Err(TestFailure::CompletedRequestLogChanged);
    }
    Ok(())
}

#[tokio::test]
async fn authenticated_request_records_context_on_the_http_root_span() -> Result<(), TestFailure> {
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    let operator_id = seed_operator(&fixture.database, LOGIN_NAME, PASSWORD).await?;
    let application = router(server_state(fixture.database.clone())?, unused_web_root());
    let login_response = drive(&application, login_request(LOGIN_NAME, PASSWORD)?).await?;
    if login_response.status != StatusCode::OK {
        return Err(TestFailure::ValidLoginFailed);
    }
    let cookie_pair = header_text(&login_response.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or(TestFailure::CookieContractChanged)?
        .to_owned();

    let captured_context = CapturedRequestContext::default();
    let subscriber = Registry::default().with(captured_context.clone());
    let response = async {
        drive(
            &application,
            cookie_request(Method::GET, "/api/v2/session", &cookie_pair)?,
        )
        .await
    }
    .with_subscriber(subscriber)
    .await?;
    if response.status != StatusCode::OK {
        return Err(TestFailure::ValidLoginFailed);
    }

    let context = captured_context.snapshot();
    let expected_actor_id = operator_id.to_string();
    if context.span_count != 1
        || context.span_name != Some("http_request")
        || context.http_method.as_deref() != Some("GET")
        || context.http_route.as_deref() != Some("/api/v2/session")
        || context.http_status != Some(200)
        || context.outcome.as_deref() != Some("success")
        || context.otel_status.is_some()
        || context.actor_kind.as_deref() != Some("operator")
        || context.actor_id.as_deref() != Some(expected_actor_id.as_str())
    {
        return Err(TestFailure::RequestContextSpanChanged);
    }
    Ok(())
}

#[tokio::test]
async fn http_root_span_records_success_client_error_and_server_error() -> Result<(), TestFailure> {
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let application = Router::new()
        .route("/success", axum::routing::get(|| async { StatusCode::OK }))
        .route(
            "/client-error",
            axum::routing::get(|| async { StatusCode::BAD_REQUEST }),
        )
        .route(
            "/server-error",
            axum::routing::get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        )
        .layer(axum_middleware::from_fn(request_context));

    for (path, status, outcome, otel_status) in [
        ("/success", StatusCode::OK, "success", None),
        (
            "/client-error",
            StatusCode::BAD_REQUEST,
            "client_error",
            None,
        ),
        (
            "/server-error",
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            Some("ERROR"),
        ),
    ] {
        let captured_context = CapturedRequestContext::default();
        let subscriber = Registry::default().with(captured_context.clone());
        let response = async { drive(&application, request(Method::GET, path, "")?).await }
            .with_subscriber(subscriber)
            .await?;
        let context = captured_context.snapshot();
        if response.status != status
            || context.span_count != 1
            || context.http_status != Some(u64::from(status.as_u16()))
            || context.outcome.as_deref() != Some(outcome)
            || context.otel_status.as_deref() != otel_status
        {
            return Err(TestFailure::RequestContextSpanChanged);
        }
    }
    Ok(())
}

#[tokio::test]
async fn http_root_span_inherits_the_remote_w3c_trace_parent() -> Result<(), TestFailure> {
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let exporter = CapturedSpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("http-trace-parent-test");
    let subscriber = Registry::default().with(
        tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_filter(tracing::level_filters::LevelFilter::TRACE),
    );
    let application = Router::new()
        .route(
            "/trace-parent",
            axum::routing::get(|| async { StatusCode::NO_CONTENT }),
        )
        .layer(axum_middleware::from_fn(request_context));
    let request = Request::builder()
        .method(Method::GET)
        .uri("/trace-parent")
        .header(
            "traceparent",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        )
        .body(Body::empty())
        .map_err(|_| TestFailure::RequestBuildFailed)?;

    let response = async { drive(&application, request).await }
        .with_subscriber(subscriber)
        .await?;
    let spans = exporter.snapshot();
    let request_span = spans
        .iter()
        .find(|span| span.name == "GET /trace-parent")
        .ok_or(TestFailure::TraceContextSpanChanged)?;
    if response.status != StatusCode::NO_CONTENT
        || request_span.span_context.trace_id().to_string() != "0af7651916cd43dd8448eb211c80319c"
        || request_span.parent_span_id.to_string() != "b7ad6b7169203331"
        || !request_span.parent_span_is_remote
        || request_span.span_kind != SpanKind::Server
    {
        return Err(TestFailure::TraceContextSpanChanged);
    }
    provider
        .shutdown()
        .map_err(|_| TestFailure::TraceContextSpanChanged)?;
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct CapturedSpanExporter {
    spans: Arc<Mutex<Vec<SpanData>>>,
}

impl CapturedSpanExporter {
    fn snapshot(&self) -> Vec<SpanData> {
        self.spans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl SpanExporter for CapturedSpanExporter {
    fn export(
        &self,
        batch: Vec<SpanData>,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        let spans = Arc::clone(&self.spans);
        async move {
            spans
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(batch);
            Ok(())
        }
    }
}

#[tokio::test]
async fn login_and_error_logs_enforce_the_redaction_contract() -> Result<(), TestFailure> {
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(&fixture.database, LOG_LOGIN_NAME, LOG_PASSWORD).await?;
    let application = router(server_state(fixture.database.clone())?, unused_web_root());
    let captured = CapturedLogs::default();
    let subscriber = captured.subscriber(LogLevel::Trace);
    let credential = async {
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
        let _authentication_failure =
            drive(&application, request(Method::GET, "/api/v2/session", "")?).await?;
        let _internal_response =
            ApiError::internal_error("test_internal_cause_canary").into_response();
        Ok(credential)
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
    let authentication_error_logged = output.lines().any(|line| {
        line.contains("WARN")
            && line.contains("HTTP request rejected")
            && line.contains("code=\"AUTHENTICATION_FAILED\"")
    });
    let internal_error_logged = output.lines().any(|line| {
        line.contains("ERROR")
            && line.contains("HTTP request failed")
            && line.contains("code=\"INTERNAL_ERROR\"")
    });
    if uppercase_output.contains("SELECT")
        || uppercase_output.contains("INSERT")
        || !authentication_error_logged
        || !internal_error_logged
        || output.contains("correlation_id")
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
    seed_operator(&fixture.database, LOGIN_NAME, PASSWORD).await?;
    let application = router(server_state(fixture.database.clone())?, unused_web_root());
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
    seed_operator(&fixture.database, LOGIN_NAME, PASSWORD).await?;
    let application = router(server_state(fixture.database.clone())?, unused_web_root());
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
    let session_count_before = session_count_on(&mut observer)?;
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
        if response.headers.contains_key("x-correlation-id") {
            return Err(TestFailure::CorrelationContractWasRetained);
        }
    }
    if session_count_on(&mut observer)? != session_count_before
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
    seed_operator(&fixture.database, LOGIN_NAME, PASSWORD).await?;
    let application = router(server_state(fixture.database.clone())?, unused_web_root());
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
            if response.headers.contains_key("x-correlation-id") {
                return Err(TestFailure::CorrelationContractWasRetained);
            }
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

fn session_count_on(connection: &mut SqliteConnection) -> Result<i64, TestFailure> {
    diesel::sql_query("SELECT COUNT(*) AS sessions FROM operator_sessions")
        .get_result::<PersistenceCountsRow>(connection)
        .map(|row| row.sessions)
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)
}

#[derive(QueryableByName)]
struct PersistenceCountsRow {
    #[diesel(sql_type = BigInt)]
    sessions: i64,
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
    #[snafu(display("the HTTP request context span contract changed"))]
    RequestContextSpanChanged,
    #[snafu(display("the W3C remote parent trace context was not inherited"))]
    TraceContextSpanChanged,
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
    #[snafu(display("the removed HTTP correlation contract was retained"))]
    CorrelationContractWasRetained,
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
