use std::{task::Poll, time::Duration};

use axum::{
    body::{Body, Bytes},
    http::{Method, Request, StatusCode, header},
};
use diesel::connection::SimpleConnection as _;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{
    Captured, LOGIN_NAME, PASSWORD, PasswordVerificationTestGuard, TestDatabase, drive,
    header_text, login_request, router, seed_operator, server_state, unused_web_root,
};
use crate::{
    component::print::{
        DOCUMENT_LIMIT,
        tests::{pdf, pdf_with_size},
    },
    db::PersistenceError,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct Fixture {
    database: TestDatabase,
    router: axum::Router,
    cookie: String,
    token: String,
    source_id: String,
}

impl Fixture {
    async fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let database = TestDatabase::new().await?;
        seed_operator(&database.database, LOGIN_NAME, PASSWORD).await?;
        database.database.write(|tx| tx.connection().batch_execute("INSERT INTO accounts VALUES ('account-1','team-1',1); INSERT INTO organizations VALUES (1,'school','','School','CHN'); INSERT INTO teams VALUES ('account-1',1,'队伍','Team','participant');").map_err(|_| PersistenceError::OperationFailed)).await.map_err(|e| format!("roster: {e:?}"))?;
        let router = router(server_state(&database)?, unused_web_root());
        let login = drive(&router, login_request(LOGIN_NAME, PASSWORD)?).await?;
        let cookie = header_text(&login.headers, &header::SET_COOKIE)?
            .split(';')
            .next()
            .ok_or("cookie")?
            .to_owned();
        let source = drive(
            &router,
            Request::builder()
                .method(Method::POST)
                .uri("/api/v2/print/sources")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"name":"domjudge-primary"}).to_string()))?,
        )
        .await?;
        assert_eq!(source.status, StatusCode::OK);
        let value: Value = serde_json::from_slice(&source.body)?;
        Ok(Self {
            database,
            router,
            cookie,
            token: value["token"].as_str().ok_or("token")?.to_owned(),
            source_id: value["source_id"].as_str().ok_or("source_id")?.to_owned(),
        })
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        content_type: &str,
        body: Body,
    ) -> Result<Captured, Box<dyn std::error::Error + Send + Sync>> {
        let response = drive(
            &self.router,
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
                .header(header::CONTENT_TYPE, content_type)
                .body(body)?,
        )
        .await?;
        assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
        Ok(response)
    }

    async fn register(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let id = Uuid::now_v7().to_string();
        let mut unexpected = registration(&id);
        unexpected["pages"] = json!(1);
        let rejected = self
            .send(
                Method::POST,
                "/api/v2/print/requests",
                "application/json",
                Body::from(unexpected.to_string()),
            )
            .await?;
        assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
        let response = self
            .send(
                Method::POST,
                "/api/v2/print/requests",
                "application/json",
                Body::from(registration(&id).to_string()),
            )
            .await?;
        assert_eq!(response.status, StatusCode::OK);
        let fact: Value = serde_json::from_slice(&response.body)?;
        assert_eq!(fact["state"], "preparing");
        for field in ["seat_label", "document", "admission", "generation_failure"] {
            assert_eq!(fact.get(field), Some(&Value::Null));
        }
        Ok(id)
    }
}

fn registration(id: &str) -> Value {
    json!({"protocol_version":1,"request_id":id,"account":"team-1","original_filename":"main.cpp","source_sha256":"12".repeat(32)})
}

fn unread_body() -> Body {
    Body::from_stream(futures_util::stream::poll_fn(
        |_| -> Poll<Option<Result<Bytes, std::io::Error>>> {
            panic!("unauthenticated print body was polled")
        },
    ))
}

fn streamed_body(bytes: &[u8]) -> Body {
    let chunks: Vec<_> = bytes.chunks(4096).map(Bytes::copy_from_slice).collect();
    Body::from_stream(futures_util::stream::iter(
        chunks.into_iter().map(Ok::<_, std::io::Error>),
    ))
}

#[tokio::test]
async fn print_sources_use_admin_authorization_dedicated_tokens_and_no_store() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    for name in [String::new(), "a".repeat(129), "judge\nsecret".to_owned()] {
        let response = drive(
            &f.router,
            Request::builder()
                .method(Method::POST)
                .uri("/api/v2/print/sources")
                .header(header::COOKIE, &f.cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"name":name}).to_string()))?,
        )
        .await?;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
    }
    let source = Uuid::parse_str(&f.source_id)?;
    assert_eq!(source.get_version_num(), 7);
    assert_eq!(f.token.len(), 5 + 36 + 1 + 64);
    assert!(f.token.starts_with(&format!("nps1.{}.", f.source_id)));
    for (method, path) in [
        (Method::POST, "/api/v2/print/requests".into()),
        (
            Method::PUT,
            format!("/api/v2/print/requests/{}/document", Uuid::now_v7()),
        ),
        (
            Method::POST,
            format!(
                "/api/v2/print/requests/{}/generation-failure",
                Uuid::now_v7()
            ),
        ),
    ] {
        for authorization in [None, Some("Bearer invalid"), Some(f.cookie.as_str())] {
            let mut builder = Request::builder()
                .method(method.clone())
                .uri(&path)
                .header(header::COOKIE, &f.cookie);
            if let Some(auth) = authorization {
                builder = builder.header(header::AUTHORIZATION, auth);
            }
            let result = drive(&f.router, builder.body(unread_body())?).await?;
            assert_eq!(result.status, StatusCode::UNAUTHORIZED);
            assert_eq!(result.headers[header::CACHE_CONTROL], "no-store");
            assert_eq!(
                serde_json::from_slice::<Value>(&result.body)?["code"],
                "AUTHENTICATION_FAILED"
            );
        }
    }
    let list = drive(
        &f.router,
        Request::builder()
            .uri("/api/v2/print/sources")
            .header(header::COOKIE, &f.cookie)
            .body(Body::empty())?,
    )
    .await?;
    let list: Value = serde_json::from_slice(&list.body)?;
    assert_eq!(list[0].as_object().ok_or("source")?.len(), 3);
    assert!(list[0].get("token").is_none() && list[0].get("token_hash").is_none());
    let denied = f
        .send(
            Method::GET,
            "/api/v2/print/sources",
            "application/json",
            Body::empty(),
        )
        .await?;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
    f.database
        .database
        .write(|tx| {
            tx.connection()
                .batch_execute("UPDATE operator_accounts SET role = 'viewer'")
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(|e| format!("role: {e:?}"))?;
    for (method, path) in [
        (Method::GET, "/api/v2/print/sources".into()),
        (Method::POST, "/api/v2/print/sources".into()),
        (
            Method::DELETE,
            format!("/api/v2/print/sources/{}", f.source_id),
        ),
    ] {
        let result = drive(
            &f.router,
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, &f.cookie)
                .body(unread_body())?,
        )
        .await?;
        assert_eq!(result.status, StatusCode::FORBIDDEN);
    }
    Ok(())
}

#[tokio::test]
async fn print_upload_exact_eight_mib_and_streamed_excess_are_endpoint_local() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let id = f.register().await?;
    let path = format!("/api/v2/print/requests/{id}/document");
    for streamed in [false, true] {
        let bytes = pdf_with_size(DOCUMENT_LIMIT);
        let response = f
            .send(
                Method::PUT,
                &path,
                "application/pdf",
                if streamed {
                    streamed_body(&bytes)
                } else {
                    Body::from(bytes)
                },
            )
            .await?;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body)?["document"]["bytes"],
            DOCUMENT_LIMIT
        );
    }
    for declared in [false, true] {
        let bytes = pdf_with_size(DOCUMENT_LIMIT + 1);
        let mut builder = Request::builder()
            .method(Method::PUT)
            .uri(&path)
            .header(header::AUTHORIZATION, format!("Bearer {}", f.token))
            .header(header::CONTENT_TYPE, "application/pdf");
        if declared {
            builder = builder.header(header::CONTENT_LENGTH, bytes.len());
        }
        let response = drive(
            &f.router,
            builder.body(if declared {
                Body::from(bytes)
            } else {
                streamed_body(&bytes)
            })?,
        )
        .await?;
        assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body)?["code"],
            "PAYLOAD_TOO_LARGE"
        );
    }
    for excess in [0, 1] {
        let mut bytes = serde_json::to_vec(&registration(&Uuid::now_v7().to_string()))?;
        bytes.resize(2 * 1024 * 1024 + excess, b' ');
        let response = f
            .send(
                Method::POST,
                "/api/v2/print/requests",
                "application/json",
                streamed_body(&bytes),
            )
            .await?;
        assert_eq!(
            response.status,
            if excess == 0 {
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            }
        );
    }
    assert_eq!(
        f.send(Method::PUT, &path, "text/plain", Body::from(pdf(1)))
            .await?
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.send(
            Method::PUT,
            &path,
            "application/pdf",
            Body::from("pdf-content-canary")
        )
        .await?
        .status,
        StatusCode::BAD_REQUEST
    );
    Ok(())
}

#[tokio::test]
async fn print_revocation_during_body_read_is_rechecked_without_body_deadline() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let id = f.register().await?;
    let path = format!("/api/v2/print/requests/{id}/document");
    let (entered, polled) = oneshot::channel();
    let (release, mut released) = oneshot::channel();
    let mut entered = Some(entered);
    let mut bytes = Some(Bytes::from(pdf(1)));
    let body = Body::from_stream(futures_util::stream::poll_fn(move |cx| {
        if let Some(entered) = entered.take() {
            let _ = entered.send(());
        }
        if bytes.is_none() {
            return Poll::Ready(None);
        }
        match std::pin::Pin::new(&mut released).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(_) => Poll::Ready(bytes.take().map(Ok::<_, std::io::Error>)),
        }
    }));
    let task = tokio::spawn(
        f.router.clone().oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(&path)
                .header(header::AUTHORIZATION, format!("Bearer {}", f.token))
                .header(header::CONTENT_TYPE, "application/pdf")
                .body(body)?,
        ),
    );
    tokio::time::timeout(Duration::from_secs(5), polled).await??;
    tokio::time::pause();
    tokio::time::advance(Duration::from_mins(1)).await;
    assert!(!task.is_finished());
    tokio::time::resume();
    let revoke_path = format!("/api/v2/print/sources/{}", f.source_id);
    for _ in 0..2 {
        let response = drive(
            &f.router,
            Request::builder()
                .method(Method::DELETE)
                .uri(&revoke_path)
                .header(header::COOKIE, &f.cookie)
                .body(Body::empty())?,
        )
        .await?;
        assert_eq!(response.status, StatusCode::NO_CONTENT);
    }
    release.send(()).map_err(|()| "body release")?;
    assert_eq!(task.await??.status(), StatusCode::UNAUTHORIZED);
    let denied = f
        .send(Method::PUT, &path, "application/pdf", unread_body())
        .await?;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
    let absent = drive(
        &f.router,
        Request::builder()
            .method(Method::DELETE)
            .uri(format!("/api/v2/print/sources/{}", Uuid::now_v7()))
            .header(header::COOKIE, &f.cookie)
            .body(Body::empty())?,
    )
    .await?;
    assert_eq!(absent.status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn print_http_registration_failure_and_recovery_follow_exact_dtos() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    for id in ["%FF", "not-a-canonical-id"] {
        let response = f
            .send(
                Method::GET,
                &format!("/api/v2/print/requests/{id}"),
                "application/json",
                Body::empty(),
            )
            .await?;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body)?["code"],
            "INVALID_REQUEST"
        );
    }
    let id = f.register().await?;
    let mut conflict = registration(&id);
    conflict["original_filename"] = "other.cpp".into();
    assert_eq!(
        f.send(
            Method::POST,
            "/api/v2/print/requests",
            "application/json",
            Body::from(conflict.to_string())
        )
        .await?
        .status,
        StatusCode::CONFLICT
    );
    for invalid in [
        json!({"protocol_version":1,"code":"compile_failed","output":"secret"}),
        json!({"protocol_version":2,"code":"compile_failed"}),
        json!({"protocol_version":1,"code":"new_error"}),
    ] {
        assert_eq!(
            f.send(
                Method::POST,
                &format!("/api/v2/print/requests/{id}/generation-failure"),
                "application/json",
                Body::from(invalid.to_string())
            )
            .await?
            .status,
            StatusCode::BAD_REQUEST
        );
    }
    let failure = f
        .send(
            Method::POST,
            &format!("/api/v2/print/requests/{id}/generation-failure"),
            "application/json",
            Body::from(json!({"protocol_version":1,"code":"compile_timeout"}).to_string()),
        )
        .await?;
    let failure: Value = serde_json::from_slice(&failure.body)?;
    assert_eq!(failure["state"], "generation_failed");
    assert_eq!(failure["generation_failure"], "compile_timeout");
    assert!(failure["document"].is_null() && failure["admission"].is_null());
    let get = f
        .send(
            Method::GET,
            &format!("/api/v2/print/requests/{id}"),
            "application/json",
            Body::empty(),
        )
        .await?;
    assert_eq!(serde_json::from_slice::<Value>(&get.body)?, failure);
    assert_eq!(
        f.send(
            Method::PUT,
            &format!("/api/v2/print/requests/{id}/document"),
            "application/pdf",
            Body::from(pdf(1))
        )
        .await?
        .status,
        StatusCode::CONFLICT
    );
    Ok(())
}

#[tokio::test]
async fn print_logs_never_include_credentials_document_bytes_or_registration_data() -> TestResult {
    use super::{CapturedLogs, LogLevel, SubscriberTestGuard};
    use tracing::instrument::WithSubscriber as _;
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let id = f.register().await?;
    let captured = CapturedLogs::default();
    async {
        let invalid = f
            .send(
                Method::PUT,
                &format!("/api/v2/print/requests/{id}/document"),
                "application/pdf",
                Body::from("PDF-private-content-canary"),
            )
            .await?;
        assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
        let mut input = registration(&Uuid::now_v7().to_string());
        input["account"] = "private-account-canary".into();
        input["original_filename"] = "private-filename-canary.cpp".into();
        let invalid = f
            .send(
                Method::POST,
                "/api/v2/print/requests",
                "application/json",
                Body::from(input.to_string()),
            )
            .await?;
        assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
        let denied = drive(
            &f.router,
            Request::builder()
                .method(Method::POST)
                .uri("/api/v2/print/requests")
                .header(header::AUTHORIZATION, format!("Bearer {}invalid", f.token))
                .body(unread_body())?,
        )
        .await?;
        assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    }
    .with_subscriber(captured.subscriber(LogLevel::Trace))
    .await?;
    let logs = captured.text().map_err(|()| "captured logs")?;
    assert!(logs.contains("print_invalid"));
    for secret in [
        &f.token,
        "PDF-private-content-canary",
        "private-account-canary",
        "private-filename-canary.cpp",
        "Authorization",
    ] {
        assert!(!logs.contains(secret));
    }
    Ok(())
}
