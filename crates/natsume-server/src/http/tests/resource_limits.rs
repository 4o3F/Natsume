use std::{future::Future as _, pin::Pin, task::Poll, time::Duration};

use axum::{
    body::{Body, Bytes},
    http::{Method, Request, StatusCode, header},
};
use tokio::sync::oneshot;
use tower::ServiceExt as _;

use super::{
    LOGIN_NAME, PASSWORD, PasswordVerificationTestGuard, TestDatabase, drive, header_text,
    login_request, router, seed_operator, server_state, unused_web_root,
};

type TestResult = Result<(), crate::test_error::TestError>;
const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

fn stalled_body() -> (Body, oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (entered, receiver) = oneshot::channel();
    let mut entered = Some(entered);
    let (disconnect, mut disconnected) = oneshot::channel();
    (
        Body::from_stream(futures_util::stream::poll_fn(move |context| {
            if let Some(entered) = entered.take() {
                let _ = entered.send(());
            }
            if Pin::new(&mut disconnected).poll(context).is_ready() {
                return Poll::Ready(Some(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "test client disconnected",
                ))));
            }
            Poll::<Option<Result<Bytes, std::io::Error>>>::Pending
        })),
        receiver,
        disconnect,
    )
}

fn unread_body() -> Body {
    Body::from_stream(futures_util::stream::poll_fn(
        |_| -> Poll<Option<Result<Bytes, std::io::Error>>> {
            panic!("rejected import request body must not be polled")
        },
    ))
}

#[tokio::test]
async fn import_rejects_busy_requests_before_buffering_and_recovers_after_body_disconnect()
-> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(&fixture.database, LOGIN_NAME, PASSWORD).await?;
    let state = server_state(&fixture)?;
    let application = router(state.clone(), unused_web_root());
    let login = drive(&application, login_request(LOGIN_NAME, PASSWORD)?).await?;
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or(crate::test_error::TestError::missing("cookie"))?;
    let commit = "/api/v2/imports/01900000-0000-7000-8000-000000000001/actions/commit";
    for path in ["/api/v2/imports", commit] {
        let (body, entered, disconnect) = stalled_body();
        let task = tokio::spawn(
            application.clone().oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .header(header::COOKIE, cookie)
                    .header(header::CONTENT_TYPE, XLSX)
                    .body(body)?,
            ),
        );
        tokio::time::timeout(Duration::from_secs(5), entered).await??;
        for path in ["/api/v2/imports", commit].into_iter().cycle().take(16) {
            let response = drive(
                &application,
                Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .header(header::COOKIE, cookie)
                    .header(header::CONTENT_TYPE, XLSX)
                    .body(unread_body())?,
            )
            .await?;
            assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(response.headers[header::RETRY_AFTER], "1");
            let error: serde_json::Value = serde_json::from_slice(&response.body)?;
            assert_eq!(error["code"], "SERVICE_UNAVAILABLE");
        }
        for path in ["/api/v2/imports", "/api/v2/imports/template"] {
            let response = drive(
                &application,
                Request::builder()
                    .uri(path)
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())?,
            )
            .await?;
            assert_eq!(response.status, StatusCode::OK);
        }
        let anonymous = drive(
            &application,
            Request::builder()
                .method(Method::POST)
                .uri("/api/v2/imports")
                .header(header::CONTENT_TYPE, XLSX)
                .body(unread_body())?,
        )
        .await?;
        assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
        task.abort();
        assert!(task.await.is_err());
        assert!(state.import().try_begin().is_err());
        assert!(disconnect.send(()).is_ok());
        tokio::time::timeout(Duration::from_secs(5), async {
            while state.import().try_begin().is_err() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
    }
    let response = drive(
        &application,
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/imports")
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, XLSX)
            .body(Body::from(
                include_bytes!("../../component/import/examples/roster.xlsx").as_slice(),
            ))?,
    )
    .await?;
    assert_eq!(response.status, StatusCode::CREATED);
    Ok(())
}
