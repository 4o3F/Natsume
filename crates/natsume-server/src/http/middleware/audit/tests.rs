use super::*;
use crate::{
    component::operator::{OperatorRole, tests::PasswordVerificationTestGuard},
    http::{
        self, AppState,
        tests::{TestDatabase, seed_operator, unused_web_root},
    },
    journal::{
        Envelope, JournalComponent,
        event::types::OperatorId,
        tests::{CapturedEvent, drain_events},
    },
    test_error::TestError,
};
use axum::{
    Router,
    body::Body,
    http::{Request as HttpRequest, header},
    middleware,
    routing::post,
};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};
use tower::ServiceExt as _;

type TestResult = Result<(), TestError>;
const PASSWORD: &str = "audit-password-secret-canary1!";
const ADMIN: &str = "audit-login-secret-canary";
const VIEWER: &str = "audit-viewer-secret-canary";

struct Fixture {
    /// Owns the temporary business database used by the real HTTP router.
    database: TestDatabase,
    /// Real composition with an explicitly injected capture channel.
    state: AppState,
    /// Mounted production router, including completion, audit and authentication.
    app: Router,
    /// Fixture-owned capture channel; no writer task is required for these assertions.
    events: mpsc::Receiver<Envelope>,
    /// Already persisted administrator used for caller attribution.
    admin_id: Uuid,
    /// Already persisted viewer used for authorization rejection coverage.
    viewer_id: Uuid,
    /// Session cookie supplied only to HTTP, never to the journal assertions as metadata.
    admin_cookie: String,
    /// Viewer session cookie supplied only to HTTP.
    viewer_cookie: String,
}

impl Fixture {
    async fn new() -> Result<Self, TestError> {
        let database = TestDatabase::new().await?;
        let admin_id = seed_operator(&database.database, ADMIN, PASSWORD).await?;
        let viewer_id = seed_operator(&database.database, VIEWER, PASSWORD).await?;
        let (journal, mut events) = JournalComponent::channel(crate::journal::CAPACITY);
        let state = Arc::new(crate::server_state::tests::for_test_with_journal(
            database.database.clone(),
            database.documents_path(),
            database.path.with_extension("logos"),
            journal,
        )?);
        state
            .operator()
            .change_role(viewer_id, OperatorRole::Viewer)
            .await?;
        let admin = state
            .operator()
            .sign_in(ADMIN, PASSWORD.into(), JournalContext::default())
            .await?;
        let viewer = state
            .operator()
            .sign_in(VIEWER, PASSWORD.into(), JournalContext::default())
            .await?;
        let admin_cookie = format!(
            "__Secure-natsume_session={}",
            admin.wire_credential().expose()
        );
        let viewer_cookie = format!(
            "__Secure-natsume_session={}",
            viewer.wire_credential().expose()
        );
        let app = http::router(state.clone(), unused_web_root());
        drain_events(&mut events);
        Ok(Self {
            database,
            state,
            app,
            events,
            admin_id,
            viewer_id,
            admin_cookie,
            viewer_cookie,
        })
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        cookie: Option<&str>,
        body: &str,
    ) -> Result<Response, TestError> {
        let mut request = HttpRequest::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        Ok(self
            .app
            .clone()
            .oneshot(request.body(Body::from(body.to_owned()))?)
            .await?)
    }

    fn records(&mut self) -> Vec<CapturedEvent> {
        drain_events(&mut self.events)
    }
}

fn request_record(
    records: &[CapturedEvent],
    route: AuditRoute,
    status: StatusCode,
) -> &CapturedEvent {
    let requests = records
        .iter()
        .filter(|record| matches!(record.event, Event::OperatorRequest(_)))
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 1, "one generic request fact");
    let record = requests[0];
    let Event::OperatorRequest(event) = &record.event else {
        panic!("request fact")
    };
    assert_eq!(event.route, route);
    assert_eq!(event.status, status.as_u16());
    assert!(record.request_id.is_some());
    for record in records {
        for secret in [
            PASSWORD,
            ADMIN,
            VIEWER,
            "nps1.token-shaped-secret-canary",
            "panic-secret-canary",
        ] {
            assert!(!record.details().contains(secret));
        }
    }
    record
}

#[tokio::test]
async fn authentication_success_and_generic_request_are_distinct_and_correlated() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;
    let body = json!({"login_name": ADMIN, "password": PASSWORD}).to_string();
    let response = f
        .request(Method::POST, "/api/v2/session", None, &body)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let records = f.records();
    let request = request_record(&records, AuditRoute::Session, StatusCode::OK);
    assert_eq!(records.len(), 2);
    let success = records
        .iter()
        .find(|record| matches!(record.event, Event::AuthSucceeded(_)))
        .ok_or(TestError::missing("auth success"))?;
    assert_eq!(success.request_id, request.request_id);
    assert_eq!(
        request.actor_operator_id.as_ref().map(OperatorId::value),
        Some(f.admin_id)
    );
    assert_eq!(success.actor_operator_id, request.actor_operator_id);
    Ok(())
}

#[tokio::test]
async fn unauthenticated_and_forbidden_requests_keep_their_error_metadata() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;
    let response = f
        .request(Method::PUT, "/api/v2/provisioning-window", None, "{}")
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let records = f.records();
    let request = request_record(&records, AuditRoute::Provisioning, StatusCode::UNAUTHORIZED);
    assert!(request.actor_operator_id.is_none());
    let details: Value = serde_json::from_str(&request.details())?;
    assert_eq!(details["api_code"], "AUTHENTICATION_FAILED");
    assert_eq!(details["cause"], "missing_session_cookie");
    let response = f
        .request(
            Method::PUT,
            "/api/v2/provisioning-window",
            Some(&f.viewer_cookie),
            "{}",
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let records = f.records();
    let request = request_record(&records, AuditRoute::Provisioning, StatusCode::FORBIDDEN);
    assert_eq!(
        request.actor_operator_id.as_ref().map(OperatorId::value),
        Some(f.viewer_id)
    );
    let details: Value = serde_json::from_str(&request.details())?;
    assert_eq!(details["api_code"], "AUTHORIZATION_DENIED");
    assert_eq!(details["cause"], "operator_authorization_denied");
    Ok(())
}

#[tokio::test]
async fn malformed_body_and_secret_shaped_path_values_never_enter_details() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;
    let response = f
        .request(
            Method::PUT,
            "/api/v2/provisioning-window",
            Some(&f.admin_cookie),
            "{broken password-secret-canary",
        )
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let records = f.records();
    request_record(&records, AuditRoute::Provisioning, StatusCode::BAD_REQUEST);
    for path in ["nps1.token-shaped-secret-canary", "%FF", "not-a-uuid"] {
        let uri = format!("/api/v2/devices/{path}");
        let response = f
            .request(Method::PATCH, &uri, Some(&f.admin_cookie), "{}")
            .await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let records = f.records();
        let record = request_record(&records, AuditRoute::Device, StatusCode::BAD_REQUEST);
        let details: Value = serde_json::from_str(&record.details())?;
        assert_eq!(details["targets"], json!([]));
        assert!(!record.details().contains(path));
    }
    Ok(())
}

#[tokio::test]
async fn committed_result_targets_and_operation_ids_are_bounded_and_correlated() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;
    let operation = Uuid::now_v7();
    let devices = (0..40)
        .map(|_| Uuid::now_v7().to_string())
        .collect::<Vec<_>>();
    let body = json!({"operation_id": operation, "scope": {"kind":"devices", "device_ids":devices}, "action":{"kind":"reset_home"}}).to_string();
    let response = f
        .request(
            Method::POST,
            "/api/v2/target-submissions",
            Some(&f.admin_cookie),
            &body,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let records = f.records();
    let request = request_record(&records, AuditRoute::TargetSubmission, StatusCode::OK);
    let details: Value = serde_json::from_str(&request.details())?;
    assert_eq!(
        details["targets"]
            .as_array()
            .ok_or(TestError::missing("targets"))?
            .len(),
        MAX_TARGETS
    );
    assert!(request.details().len() <= 2048);
    assert_eq!(
        request.operation_id,
        Some(OperationId::from_operation(operation))
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record.event, Event::TargetSubmitted(_)))
            .count(),
        40
    );
    assert!(
        records
            .iter()
            .all(|record| record.request_id == request.request_id)
    );
    Ok(())
}

#[tokio::test]
async fn panic_records_one_redacted_internal_error_without_changing_completion() -> TestResult {
    async fn panics() -> StatusCode {
        panic!("panic-secret-canary")
    }
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;

    let app = Router::new()
        .route("/session", post(panics))
        .layer(middleware::from_fn_with_state(
            f.state.journal().emitter(),
            audit_request,
        ))
        .layer(middleware::from_fn_with_state(
            super::super::CompletionState::new(
                f.state.lifecycle().clone(),
                f.state.journal().emitter(),
            ),
            super::super::complete_mutation,
        ));
    let response = app
        .oneshot(HttpRequest::post("/session").body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let records = f.records();
    let request = request_record(
        &records,
        AuditRoute::Session,
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    let details: Value = serde_json::from_str(&request.details())?;
    assert_eq!(details["api_code"], "INTERNAL_ERROR");
    assert_eq!(details["cause"], "request_task_failed");
    Ok(())
}

#[derive(Clone)]
struct BlockingMutation {
    /// Confirms the admitted handler has begun before its caller is cancelled.
    entered: Arc<Notify>,
    /// Allows the handler to finish after the caller has already disappeared.
    release: Arc<Notify>,
}

#[tokio::test]
async fn cancelled_caller_still_records_exactly_one_completed_request() -> TestResult {
    async fn blocked(State(blocking): State<BlockingMutation>) -> StatusCode {
        blocking.entered.notify_one();
        blocking.release.notified().await;
        StatusCode::NO_CONTENT
    }
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;

    let blocking = BlockingMutation {
        entered: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    };
    let app = Router::new()
        .route("/session", post(blocked))
        .with_state(blocking.clone())
        .layer(middleware::from_fn_with_state(
            f.state.journal().emitter(),
            audit_request,
        ))
        .layer(middleware::from_fn_with_state(
            super::super::CompletionState::new(
                f.state.lifecycle().clone(),
                f.state.journal().emitter(),
            ),
            super::super::complete_mutation,
        ));
    let caller = tokio::spawn(app.oneshot(HttpRequest::post("/session").body(Body::empty())?));
    blocking.entered.notified().await;
    caller.abort();
    f.state.lifecycle().begin_shutdown();
    blocking.release.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.state.lifecycle().wait(),
    )
    .await?;
    let records = f.records();
    request_record(&records, AuditRoute::Session, StatusCode::NO_CONTENT);
    assert_eq!(records.len(), 1);
    Ok(())
}

#[tokio::test]
async fn shutdown_refusals_are_audited_before_authentication() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;
    f.state.lifecycle().begin_shutdown();
    let response = f
        .request(Method::POST, "/api/v2/session", Some(&f.admin_cookie), "{}")
        .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let records = f.records();
    let request = request_record(
        &records,
        AuditRoute::Session,
        StatusCode::SERVICE_UNAVAILABLE,
    );
    assert!(request.actor_operator_id.is_none());
    let details: Value = serde_json::from_str(&request.details())?;
    assert_eq!(details["api_code"], "SERVICE_UNAVAILABLE");
    assert_eq!(details["cause"], "server_stopping");
    Ok(())
}

#[tokio::test]
async fn canonical_path_targets_are_typed_and_query_strings_are_ignored() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;
    let device_id = Uuid::now_v7();
    let uri = format!("/api/v2/devices/{device_id}?secret=nps1.token-shaped-secret-canary");
    let response = f
        .request(
            Method::PATCH,
            &uri,
            Some(&f.admin_cookie),
            "{\"state\":\"enabled\"}",
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let records = f.records();
    let request = request_record(&records, AuditRoute::Device, StatusCode::NOT_FOUND);
    let details: Value = serde_json::from_str(&request.details())?;
    assert_eq!(details["targets"], json!([{ "device": device_id }]));
    assert_eq!(details["api_code"], "RESOURCE_NOT_FOUND");
    assert_eq!(
        request.actor_operator_id.as_ref().map(OperatorId::value),
        Some(f.admin_id)
    );
    Ok(())
}

#[tokio::test]
async fn read_and_unmatched_requests_are_silent_and_operator_method_refusals_are_audited()
-> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;
    f.request(Method::GET, "/api/v2/session", Some(&f.admin_cookie), "")
        .await?;
    f.request(Method::HEAD, "/api/v2/session", Some(&f.admin_cookie), "")
        .await?;
    assert!(f.records().is_empty());
    let response = f
        .request(
            Method::POST,
            "/api/v2/unknown/nps1.token-shaped-secret-canary",
            None,
            "{}",
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(f.records().is_empty());
    let response = f
        .request(Method::OPTIONS, "/api/v2/provisioning-window", None, "")
        .await?;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let records = f.records();
    let record = request_record(
        &records,
        AuditRoute::Provisioning,
        StatusCode::METHOD_NOT_ALLOWED,
    );
    assert!(
        matches!(&record.event, Event::OperatorRequest(event) if event.method == HttpMethod::Options)
    );
    Ok(())
}

#[tokio::test]
async fn failed_session_polling_is_silent_and_mutating_refusals_only_emit_requests() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;
    crate::component::operator::tests::test_expire_all_sessions(&f.database.database).await?;
    for cookie in [
        &f.admin_cookie,
        "__Secure-natsume_session=nps1.token-shaped-secret-canary",
    ] {
        let response = f
            .request(Method::GET, "/api/v2/session", Some(cookie), "")
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert!(f.records().is_empty());
    let response = f
        .request(
            Method::PUT,
            "/api/v2/provisioning-window",
            Some(&f.admin_cookie),
            "{}",
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let records = f.records();
    request_record(&records, AuditRoute::Provisioning, StatusCode::UNAUTHORIZED);
    assert_eq!(records.len(), 1);
    Ok(())
}

#[tokio::test]
async fn machine_print_requests_are_not_audited_even_during_shutdown() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;
    for path in [
        "/api/v2/print/client/claims",
        "/api/v2/print/client/pair",
        "/api/v2/print/client/confirmation",
        "/api/v2/print/requests",
        "/api/v2/print/requests/nps1.token-shaped-secret-canary/generation-failure",
    ] {
        let response = f.request(Method::POST, path, None, "{}").await?;
        let expected_status = if path == "/api/v2/print/client/pair" {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::UNAUTHORIZED
        };
        assert_eq!(response.status(), expected_status, "{path}");
    }
    assert!(f.records().is_empty());
    f.state.lifecycle().begin_shutdown();
    let response = f
        .request(Method::POST, "/api/v2/print/client/claims", None, "{}")
        .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(f.records().is_empty());
    Ok(())
}

/// Valid remote parent propagated by the production request-span middleware.
const TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

fn telemetry_dispatch() -> tracing::Dispatch {
    use opentelemetry::trace::TracerProvider as _;
    use tracing_subscriber::prelude::*;
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    tracing::Dispatch::new(
        tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("audit-test"))),
    )
}

#[tokio::test]
async fn request_span_trace_is_shared_by_auth_domain_and_request_events() -> TestResult {
    use tracing::instrument::WithSubscriber as _;
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;
    let dispatcher = telemetry_dispatch();
    let sign_in = HttpRequest::post("/api/v2/session")
        .header(header::CONTENT_TYPE, "application/json")
        .header("traceparent", TRACEPARENT)
        .body(Body::from(
            json!({"login_name": ADMIN, "password": PASSWORD}).to_string(),
        ))?;
    let response = f
        .app
        .clone()
        .oneshot(sign_in)
        .with_subscriber(dispatcher.clone())
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let records = f.records();
    assert_eq!(records.len(), 2);
    for record in &records {
        assert_eq!(
            record.trace_id.map(|id| id.to_string()).as_deref(),
            Some("0af7651916cd43dd8448eb211c80319c")
        );
    }
    let request = HttpRequest::post("/api/v2/target-submissions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, &f.admin_cookie)
        .header("traceparent", TRACEPARENT)
        .body(Body::from(json!({"operation_id": Uuid::now_v7(), "scope": {"kind":"devices", "device_ids":[Uuid::now_v7()]}, "action":{"kind":"reset_home"}}).to_string()))?;
    let response = f
        .app
        .clone()
        .oneshot(request)
        .with_subscriber(dispatcher)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let records = f.records();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .any(|record| matches!(record.event, Event::TargetSubmitted(_)))
    );
    for record in &records {
        assert_eq!(
            record.trace_id.map(|id| id.to_string()).as_deref(),
            Some("0af7651916cd43dd8448eb211c80319c")
        );
    }
    let response = f
        .request(Method::PUT, "/api/v2/provisioning-window", None, "{}")
        .with_subscriber(tracing::subscriber::NoSubscriber::default())
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let records = f.records();
    assert_eq!(records.len(), 1);
    assert!(records[0].trace_id.is_none());
    Ok(())
}

#[tokio::test]
async fn panic_and_shutdown_refusal_keep_the_captured_request_trace() -> TestResult {
    use tracing::instrument::WithSubscriber as _;
    async fn panics() -> StatusCode {
        panic!("panic-secret-canary")
    }
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;
    let app = Router::new()
        .route("/session", post(panics))
        .layer(middleware::from_fn_with_state(
            f.state.journal().emitter(),
            audit_request,
        ))
        .layer(middleware::from_fn_with_state(
            super::super::CompletionState::new(
                f.state.lifecycle().clone(),
                f.state.journal().emitter(),
            ),
            super::super::complete_mutation,
        ))
        .layer(middleware::from_fn(super::super::request_context));
    let dispatcher = telemetry_dispatch();
    let request = HttpRequest::post("/session")
        .header("traceparent", TRACEPARENT)
        .body(Body::empty())?;
    let response = app
        .oneshot(request)
        .with_subscriber(dispatcher.clone())
        .await?;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let records = f.records();
    let record = request_record(
        &records,
        AuditRoute::Session,
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    assert_eq!(
        record.trace_id.map(|id| id.to_string()).as_deref(),
        Some("0af7651916cd43dd8448eb211c80319c")
    );
    f.state.lifecycle().begin_shutdown();
    let request = HttpRequest::post("/api/v2/session")
        .header("traceparent", TRACEPARENT)
        .body(Body::empty())?;
    let response = f
        .app
        .clone()
        .oneshot(request)
        .with_subscriber(dispatcher)
        .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let records = f.records();
    let record = request_record(
        &records,
        AuditRoute::Session,
        StatusCode::SERVICE_UNAVAILABLE,
    );
    assert_eq!(
        record.trace_id.map(|id| id.to_string()).as_deref(),
        Some("0af7651916cd43dd8448eb211c80319c")
    );
    Ok(())
}

#[tokio::test]
async fn photo_administrator_mutations_are_audited_without_credentials() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let mut f = Fixture::new().await?;
    let name = "photo-source-name-secret-canary";
    let response = f
        .request(
            Method::POST,
            "/api/v2/photo/sources",
            Some(&f.admin_cookie),
            &json!({"name":name}).to_string(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let source: Value =
        serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 8192).await?)?;
    let id = source["source_id"]
        .as_str()
        .ok_or(TestError::missing("photo source id"))?;
    let records = f.records();
    let record = request_record(&records, AuditRoute::PhotoSources, StatusCode::CREATED);
    assert_eq!(
        record.actor_operator_id.as_ref().map(OperatorId::value),
        Some(f.admin_id)
    );
    let details: Value = serde_json::from_str(&record.details())?;
    assert_eq!(details["targets"], json!([{"photo_source":id}]));
    assert!(!record.details().contains(name));
    let pairing_path = format!("/api/v2/photo/sources/{id}/pairing-code");
    let response = f
        .request(
            Method::POST,
            &pairing_path,
            Some(&f.admin_cookie),
            "{\"replace_pending_enrollment\":true}",
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let issued: Value =
        serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 8192).await?)?;
    let code = issued["code"]
        .as_str()
        .ok_or(TestError::missing("pairing code"))?;
    let records = f.records();
    let record = request_record(&records, AuditRoute::PhotoPairingCode, StatusCode::OK);
    assert!(!record.details().contains(code));
    let details: Value = serde_json::from_str(&record.details())?;
    assert_eq!(details["targets"], json!([{"photo_source":id}]));
    let state_path = format!("/api/v2/photo/sources/{id}/state");
    let response = f
        .request(
            Method::POST,
            &state_path,
            Some(&f.admin_cookie),
            "{\"state\":\"disabled\"}",
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    request_record(
        &f.records(),
        AuditRoute::PhotoSourceState,
        StatusCode::NO_CONTENT,
    );
    for (cookie, status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(f.viewer_cookie.clone()), StatusCode::FORBIDDEN),
    ] {
        let response = f
            .request(Method::POST, &pairing_path, cookie.as_deref(), "{}")
            .await?;
        assert_eq!(response.status(), status);
        request_record(&f.records(), AuditRoute::PhotoPairingCode, status);
    }
    f.request(
        Method::GET,
        "/api/v2/photo/sources",
        Some(&f.admin_cookie),
        "",
    )
    .await?;
    assert!(f.records().is_empty());
    Ok(())
}

#[tokio::test]
async fn photo_client_enrollment_is_excluded_from_operator_audit() -> TestResult {
    let mut f = Fixture::new().await?;
    for path in [
        "/api/v2/photo/enrollment/pair",
        "/api/v2/photo/enrollment/confirm",
        "/api/v2/photo/client/initialization/complete",
    ] {
        f.request(
            Method::POST,
            path,
            None,
            "{\"token\":\"photo-secret-canary\"}",
        )
        .await?;
        assert!(
            f.records().is_empty(),
            "Photo Client routes are outside Operator audit scope"
        );
    }
    Ok(())
}
