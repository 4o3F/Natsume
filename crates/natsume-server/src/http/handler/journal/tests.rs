use super::*;
use crate::{
    component::operator::tests::PasswordVerificationTestGuard,
    journal::{
        event::{Event, payload::LeaseAttached, types::SessionId},
        read::{Filter, ReadRequest},
        tests::corrupt_latest,
    },
    journal_query::{Visibility, tests::Fixture},
    test_error::TestError,
};
use axum::{body::to_bytes, http::Request};
use tower::ServiceExt as _;
use uuid::Uuid;

async fn request(f: &Fixture, path: &str, cookie: Option<&str>) -> Result<Response, TestError> {
    let mut request = Request::get(path);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    Ok(crate::http::router(
        f.state.clone(),
        std::path::Path::new("/nonexistent/journal-web"),
    )
    .oneshot(request.body(Body::empty())?)
    .await?)
}
async fn body(response: Response) -> Result<String, TestError> {
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024).await?;
    Ok(std::str::from_utf8(&bytes)?.to_owned())
}

#[tokio::test]
async fn every_endpoint_has_operator_permissions_no_store_and_no_get_audit() -> Result<(), TestError>
{
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    for path in [
        "/api/v2/journal/events".to_owned(),
        format!(
            "/api/v2/journal/devices/{}/timeline?from=0&to=100",
            f.device_id.as_text()
        ),
        "/api/v2/journal/device-summaries?window_ms=1000".into(),
        "/api/v2/journal/status".into(),
        "/api/v2/journal/export".into(),
    ] {
        let response = request(&f, &path, None).await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let response = request(&f, &path, Some(&f.viewer_cookie)).await?;
        let expected = if path.ends_with("status") || path.ends_with("export") {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::OK
        };
        assert_eq!(response.status(), expected, "{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let response = request(&f, &path, Some(&f.admin_cookie)).await?;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        body(response).await?;
    }
    let audit = f
        .state
        .journal_query()
        .events(
            ReadRequest {
                filter: Filter {
                    kinds: vec!["operator_request".into()],
                    ..Filter::default()
                },
                limit: 1000,
                ..ReadRequest::default()
            },
            Visibility::All,
        )
        .await?;
    assert!(audit.events.is_empty());
    f.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn viewer_restrictions_and_malformed_inputs_never_echo_path_or_query_text()
-> Result<(), TestError> {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    for query in [
        "category[]=auth",
        "category[]=device&category[]=audit",
        "kind[]=auth_succeeded",
        "kind[]=server_started",
    ] {
        let response = request(
            &f,
            &format!("/api/v2/journal/events?{query}"),
            Some(&f.viewer_cookie),
        )
        .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    for path in [
        "/api/v2/journal/events?device_id=nps1.token-secret-canary",
        "/api/v2/journal/events?kind[]=password-secret-canary",
        "/api/v2/journal/events?before=token-secret-canary",
        "/api/v2/journal/devices/nps1.token-secret-canary/timeline?from=0&to=100",
        "/api/v2/journal/devices/%FF/timeline?from=0&to=100",
        "/api/v2/journal/events?from=20&to=10",
        "/api/v2/journal/events?limit=0",
        "/api/v2/journal/events?from=1&from=2",
        "/api/v2/journal/device-summaries?window_ms=0",
    ] {
        let response = request(&f, path, Some(&f.admin_cookie)).await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = body(response).await?;
        assert!(!body.contains("secret-canary"));
    }
    f.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn unreadable_rows_are_bounded_and_export_never_echoes_secrets() -> Result<(), TestError> {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    f.seed(vec![f.row(
        10,
        Event::LeaseAttached(LeaseAttached {
            session_id: SessionId::from_lease(Uuid::now_v7()),
        }),
    )])
    .await?;
    corrupt_latest(f.state.journal(), "password_token_login_secret_canary").await?;
    for path in ["/api/v2/journal/events", "/api/v2/journal/export"] {
        let response = request(&f, path, Some(&f.admin_cookie)).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body(response).await?;
        for secret in [
            "password_token_login_secret_canary",
            "journal-password-secret-canary1!",
            "nps1.token-secret-canary",
        ] {
            assert!(!body.contains(secret));
        }
        assert!(body.contains("unreadable"));
    }
    f.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn rejected_read_canaries_never_reach_the_journal_database_or_sidecars()
-> Result<(), TestError> {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    for path in [
        "/api/v2/journal/events?device_id=attempted-login-secret-canary",
        "/api/v2/journal/events?operation_id=password-secret-canary",
        "/api/v2/journal/events?request_id=nps1.token-secret-canary",
    ] {
        let response = request(&f, path, Some(&f.admin_cookie)).await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    crate::journal::tests::ready(f.state.journal()).await?;
    for file in ["journal.db", "journal.db-wal", "journal.db-shm"] {
        let path = f.directory.path().join(file);
        match std::fs::read(path) {
            Ok(bytes) => {
                for secret in [
                    "attempted-login-secret-canary",
                    "password-secret-canary",
                    "nps1.token-secret-canary",
                    "journal-password-secret-canary1!",
                ] {
                    assert!(
                        !bytes
                            .windows(secret.len())
                            .any(|window| window == secret.as_bytes()),
                        "secret in {file}"
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    f.state.journal().shutdown().await;
    Ok(())
}
