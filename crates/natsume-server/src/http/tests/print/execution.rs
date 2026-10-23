use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};
use snafu::Snafu;
use uuid::Uuid;

use super::{Fixture, PasswordVerificationTestGuard, drive, pdf, unread_body};
use crate::http::tests::{Captured, SupportFailure};

#[derive(Debug, Snafu)]
enum ExecutionTestError {
    #[snafu(display("print execution fixture setup failed"))]
    Setup,
    #[snafu(context(false), display("print execution request failed"))]
    Request { source: axum::http::Error },
    #[snafu(context(false), display("print execution response failed"))]
    Response { source: SupportFailure },
    #[snafu(context(false), display("print execution JSON failed"))]
    Json { source: serde_json::Error },
}

type TestResult = Result<(), ExecutionTestError>;

async fn send(
    f: &Fixture,
    token: &str,
    method: Method,
    path: &str,
    value: Value,
) -> Result<Captured, ExecutionTestError> {
    let result = drive(
        &f.router,
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))?,
    )
    .await?;
    assert_eq!(result.headers[header::CACHE_CONTROL], "no-store");
    Ok(result)
}

async fn paired(f: &Fixture, confirm: bool) -> Result<Value, ExecutionTestError> {
    let mut value = Value::Null;
    for (path, body) in [
        (
            "/api/v2/print/printers".to_owned(),
            json!({"name":"Printer"}),
        ),
        (String::new(), Value::Null),
    ] {
        let path = if path.is_empty() {
            format!(
                "/api/v2/print/printers/{}/pairing-code",
                value["printer_id"]
                    .as_str()
                    .ok_or(ExecutionTestError::Setup)?
            )
        } else {
            path
        };
        let response = drive(
            &f.router,
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(header::COOKIE, &f.cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))?,
        )
        .await?;
        assert_eq!(response.status, StatusCode::OK);
        value = serde_json::from_slice(&response.body)?;
    }
    let response = drive(
        &f.router,
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/print/client/pair")
            .extension(axum::extract::ConnectInfo(crate::tls::PeerAddress(
                "192.0.2.10"
                    .parse()
                    .map_err(|_| ExecutionTestError::Setup)?,
            )))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"code":value["code"]}).to_string()))?,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&response.body)?;
    if confirm {
        let response = send(
            f,
            value["token"].as_str().ok_or(ExecutionTestError::Setup)?,
            Method::POST,
            "/api/v2/print/client/confirmation",
            Value::Null,
        )
        .await?;
        assert_eq!(response.status, StatusCode::OK);
    }
    Ok(value)
}

async fn ready(f: &Fixture) -> Result<Vec<u8>, ExecutionTestError> {
    let id = f.register().await.map_err(|_| ExecutionTestError::Setup)?;
    let bytes = pdf(2);
    let result = f
        .send(
            Method::PUT,
            &format!("/api/v2/print/requests/{id}/document"),
            "application/pdf",
            Body::from(bytes.clone()),
        )
        .await
        .map_err(|_| ExecutionTestError::Setup)?;
    assert_eq!(result.status, StatusCode::OK);
    Ok(bytes)
}

fn claim() -> Value {
    json!({"request_id":Uuid::now_v7().to_string(),"queue_name":"Windows queue"})
}

#[tokio::test]
async fn execution_rejects_wrong_identities_before_reading_bodies() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new()
        .await
        .map_err(|_| ExecutionTestError::Setup)?;
    let pending = paired(&f, false).await?;
    let id = Uuid::now_v7().to_string();
    for (method, path) in [
        (Method::POST, "/api/v2/print/client/claims".to_owned()),
        (
            Method::GET,
            "/api/v2/print/client/attempts/current".to_owned(),
        ),
        (Method::GET, format!("/api/v2/print/client/attempts/{id}")),
        (
            Method::PUT,
            format!("/api/v2/print/client/attempts/{id}/job"),
        ),
        (
            Method::POST,
            format!("/api/v2/print/client/attempts/{id}/reports"),
        ),
        (
            Method::GET,
            format!("/api/v2/print/client/attempts/{id}/document"),
        ),
    ] {
        for token in [
            None,
            Some(f.token.as_str()),
            pending["token"].as_str(),
            Some("npc1.invalid.invalid"),
        ] {
            let mut request = Request::builder()
                .method(method.clone())
                .uri(&path)
                .header(header::COOKIE, &f.cookie);
            if let Some(token) = token {
                request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
            }
            let response = drive(&f.router, request.body(unread_body())?).await?;
            assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{method} {path}");
            assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
        }
    }
    Ok(())
}

async fn assert_owned_document(
    f: &Fixture,
    token: &str,
    other_token: &str,
    path: &str,
    attempt: &Value,
    bytes: &[u8],
) -> TestResult {
    assert_eq!(attempt["attempt_number"], 1);
    assert!(attempt.get("protocol_version").is_none());
    for (method, path, value) in [
        (Method::GET, path.to_owned(), Value::Null),
        (Method::GET, format!("{path}/document"), Value::Null),
        (
            Method::PUT,
            format!("{path}/job"),
            json!({"job_id":42,"queue_name":"Windows queue","label":attempt["label"]}),
        ),
        (
            Method::POST,
            format!("{path}/reports"),
            json!({"kind":"failed"}),
        ),
    ] {
        assert_eq!(
            send(f, other_token, method, &path, value).await?.status,
            StatusCode::NOT_FOUND
        );
    }
    let downloaded = send(
        f,
        token,
        Method::GET,
        &format!("{path}/document"),
        Value::Null,
    )
    .await?;
    assert_eq!(downloaded.status, StatusCode::OK);
    assert_eq!(downloaded.headers[header::CONTENT_TYPE], "application/pdf");
    assert_eq!(&downloaded.body[..], bytes);
    Ok(())
}

async fn assert_printed_replay(
    f: &Fixture,
    token: &str,
    path: &str,
    attempt: &Value,
) -> TestResult {
    let job = json!({"job_id":42,"queue_name":"Windows queue","label":attempt["label"]});
    assert_eq!(
        send(f, token, Method::PUT, &format!("{path}/job"), job.clone())
            .await?
            .status,
        StatusCode::OK
    );
    assert_eq!(
        send(
            f,
            token,
            Method::POST,
            &format!("{path}/reports"),
            json!({"kind":"observed","job":job})
        )
        .await?
        .status,
        StatusCode::OK
    );
    let printed = send(
        f,
        token,
        Method::POST,
        &format!("{path}/reports"),
        json!({"kind":"printed","job":job}),
    )
    .await?;
    let result: Value = serde_json::from_slice(&printed.body)?;
    assert_eq!(result["state"], "printed");
    let replay = send(
        f,
        token,
        Method::POST,
        &format!("{path}/reports"),
        json!({"kind":"printed","job":job}),
    )
    .await?;
    assert_eq!(serde_json::from_slice::<Value>(&replay.body)?, result);
    assert_eq!(
        send(
            f,
            token,
            Method::GET,
            &format!("{path}/document"),
            Value::Null
        )
        .await?
        .status,
        StatusCode::CONFLICT
    );
    Ok(())
}

#[tokio::test]
async fn complete_http_execution_is_owned_idempotent_and_separate_from_admission() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new()
        .await
        .map_err(|_| ExecutionTestError::Setup)?;
    let client = paired(&f, true).await?;
    let other = paired(&f, true).await?;
    let token = client["token"].as_str().ok_or(ExecutionTestError::Setup)?;
    let other_token = other["token"].as_str().ok_or(ExecutionTestError::Setup)?;
    let bytes = ready(&f).await?;
    let input = claim();
    let response = send(
        &f,
        token,
        Method::POST,
        "/api/v2/print/client/claims",
        input.clone(),
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK);
    let attempt: Value = serde_json::from_slice(&response.body)?;
    assert_eq!(attempt["state"], "preparing");
    let path = format!(
        "/api/v2/print/client/attempts/{}",
        attempt["attempt_id"]
            .as_str()
            .ok_or(ExecutionTestError::Setup)?
    );
    let replay = send(
        &f,
        token,
        Method::POST,
        "/api/v2/print/client/claims",
        input,
    )
    .await?;
    assert_eq!(serde_json::from_slice::<Value>(&replay.body)?, attempt);
    assert_owned_document(&f, token, other_token, &path, &attempt, &bytes).await?;
    let binding_path = format!(
        "/api/v2/print/printers/{}/binding",
        client["printer_id"]
            .as_str()
            .ok_or(ExecutionTestError::Setup)?
    );
    let blocked = drive(
        &f.router,
        Request::builder()
            .method(Method::DELETE)
            .uri(&binding_path)
            .header(header::COOKIE, &f.cookie)
            .body(Body::empty())?,
    )
    .await?;
    assert_eq!(blocked.status, StatusCode::CONFLICT);
    assert_printed_replay(&f, token, &path, &attempt).await?;
    let executions = drive(
        &f.router,
        Request::builder()
            .uri("/api/v2/print/executions")
            .header(header::COOKIE, &f.cookie)
            .body(Body::empty())?,
    )
    .await?;
    let executions: Value = serde_json::from_slice(&executions.body)?;
    assert_eq!(executions[0]["state"], "completed");
    assert_eq!(executions[0]["last_outcome"], "printed");
    assert_eq!(
        send(
            &f,
            token,
            Method::GET,
            "/api/v2/print/executions",
            Value::Null
        )
        .await?
        .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&f, token, Method::GET, "/api/v2/print/queue", Value::Null)
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    let cleared = drive(
        &f.router,
        Request::builder()
            .method(Method::DELETE)
            .uri(&binding_path)
            .header(header::COOKIE, &f.cookie)
            .body(Body::empty())?,
    )
    .await?;
    assert_eq!(cleared.status, StatusCode::NO_CONTENT);
    assert_eq!(
        send(&f, token, Method::GET, &path, Value::Null)
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn binding_revocation_while_claim_body_is_pending_is_rechecked() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new()
        .await
        .map_err(|_| ExecutionTestError::Setup)?;
    let client = paired(&f, true).await?;
    let token = client["token"].as_str().ok_or(ExecutionTestError::Setup)?;
    ready(&f).await?;
    let (entered, polled) = tokio::sync::oneshot::channel();
    let (release, mut released) = tokio::sync::oneshot::channel();
    let mut entered = Some(entered);
    let mut bytes = Some(axum::body::Bytes::from(claim().to_string()));
    let body = Body::from_stream(futures_util::stream::poll_fn(move |cx| {
        if let Some(entered) = entered.take() {
            let _ = entered.send(());
        }
        if bytes.is_none() {
            return std::task::Poll::Ready(None);
        }
        match std::future::Future::poll(std::pin::Pin::new(&mut released), cx) {
            std::task::Poll::Pending => std::task::Poll::Pending,
            std::task::Poll::Ready(_) => {
                std::task::Poll::Ready(bytes.take().map(Ok::<_, std::io::Error>))
            }
        }
    }));
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v2/print/client/claims")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)?;
    let router = f.router.clone();
    let pending = tokio::spawn(async move { drive(&router, request).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), polled)
        .await
        .map_err(|_| ExecutionTestError::Setup)?
        .map_err(|_| ExecutionTestError::Setup)?;
    let path = format!(
        "/api/v2/print/printers/{}/binding",
        client["printer_id"]
            .as_str()
            .ok_or(ExecutionTestError::Setup)?
    );
    let result = drive(
        &f.router,
        Request::builder()
            .method(Method::DELETE)
            .uri(path)
            .header(header::COOKIE, &f.cookie)
            .body(Body::empty())?,
    )
    .await?;
    assert_eq!(result.status, StatusCode::NO_CONTENT);
    release.send(()).map_err(|()| ExecutionTestError::Setup)?;
    let response = pending.await.map_err(|_| ExecutionTestError::Setup)??;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn execution_logs_do_not_include_client_credentials_or_document_bytes() -> TestResult {
    use crate::{
        config::LogLevel,
        logging::tests::{CapturedLogs, SubscriberTestGuard},
    };
    use sha2::{Digest as _, Sha256};
    use tracing::instrument::WithSubscriber as _;
    let _subscriber = SubscriberTestGuard::acquire();
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new()
        .await
        .map_err(|_| ExecutionTestError::Setup)?;
    let capture = CapturedLogs::default();
    let subscriber = capture.subscriber(LogLevel::Trace);
    let token = async {
        let client = paired(&f, true).await?;
        let token = client["token"]
            .as_str()
            .ok_or(ExecutionTestError::Setup)?
            .to_owned();
        ready(&f).await?;
        let response = send(
            &f,
            &token,
            Method::POST,
            "/api/v2/print/client/claims",
            claim(),
        )
        .await?;
        let attempt: Value = serde_json::from_slice(&response.body)?;
        let path = format!(
            "/api/v2/print/client/attempts/{}/document",
            attempt["attempt_id"]
                .as_str()
                .ok_or(ExecutionTestError::Setup)?
        );
        assert_eq!(
            send(&f, &token, Method::GET, &path, Value::Null)
                .await?
                .status,
            StatusCode::OK
        );
        assert_eq!(
            send(
                &f,
                &f.token,
                Method::POST,
                "/api/v2/print/client/claims",
                claim()
            )
            .await?
            .status,
            StatusCode::UNAUTHORIZED
        );
        Ok::<_, ExecutionTestError>(token)
    }
    .with_subscriber(subscriber)
    .await?;
    let text = capture.text().map_err(|()| ExecutionTestError::Setup)?;
    assert!(text.contains("print_authentication"));
    for value in [
        &token,
        &hex::encode(Sha256::digest(token.as_bytes())),
        &f.token,
        "%PDF-1.7",
    ] {
        assert!(!text.contains(value));
    }
    Ok(())
}

#[tokio::test]
async fn execution_rejects_open_fields_invalid_ids_queues_jobs_and_report_shapes() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new()
        .await
        .map_err(|_| ExecutionTestError::Setup)?;
    let client = paired(&f, true).await?;
    let token = client["token"].as_str().ok_or(ExecutionTestError::Setup)?;
    for input in [
        json!({"protocol_version":1,"request_id":Uuid::now_v7().to_string(),"queue_name":"q"}),
        json!({"request_id":"invalid","queue_name":"q"}),
        json!({"request_id":Uuid::now_v7().to_string(),"queue_name":""}),
        json!({"request_id":Uuid::now_v7().to_string(),"queue_name":"q\n"}),
        json!({"request_id":Uuid::now_v7().to_string(),"queue_name":"q","extra":true}),
    ] {
        assert_eq!(
            send(
                &f,
                token,
                Method::POST,
                "/api/v2/print/client/claims",
                input
            )
            .await?
            .status,
            StatusCode::BAD_REQUEST
        );
    }
    ready(&f).await?;
    let response = send(
        &f,
        token,
        Method::POST,
        "/api/v2/print/client/claims",
        claim(),
    )
    .await?;
    let attempt: Value = serde_json::from_slice(&response.body)?;
    let path = format!(
        "/api/v2/print/client/attempts/{}",
        attempt["attempt_id"]
            .as_str()
            .ok_or(ExecutionTestError::Setup)?
    );
    for job_id in [json!(0), json!(-1), json!(4_294_967_296_i64), json!(1.5)] {
        assert_eq!(
            send(
                &f,
                token,
                Method::PUT,
                &format!("{path}/job"),
                json!({"queue_name":"Windows queue","job_id":job_id,"label":attempt["label"]})
            )
            .await?
            .status,
            StatusCode::BAD_REQUEST
        );
    }
    for body in [
        json!({"kind":"other"}),
        json!({"kind":"contact","extra":1}),
        json!({"kind":"contact","job":{"unexpected":true}}),
        json!({"kind":"observed"}),
        json!({"kind":"observed","job":{"queue_name":"Windows queue","job_id":42,"label":attempt["label"],"extra":true}}),
    ] {
        assert_eq!(
            send(&f, token, Method::POST, &format!("{path}/reports"), body)
                .await?
                .status,
            StatusCode::BAD_REQUEST
        );
    }
    Ok(())
}
