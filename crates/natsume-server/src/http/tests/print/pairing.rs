use std::net::IpAddr;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use serde_json::{Value, json};

use super::{Captured, Fixture, PasswordVerificationTestGuard, drive, unread_body};
use crate::tls::PeerAddress;

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("print pairing HTTP fixture: {error:?}"))
}

fn value(response: &Captured) -> Value {
    must(serde_json::from_slice(&response.body))
}

async fn admin(f: &Fixture, method: Method, path: &str, body: Value) -> Captured {
    let request = must(
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::COOKIE, &f.cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
    );
    let response = must(drive(&f.router, request).await);
    assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    response
}

async fn create(f: &Fixture) -> String {
    let response = admin(
        f,
        Method::POST,
        "/api/v2/print/printers",
        json!({"name":"主打印机"}),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    value(&response)["printer_id"]
        .as_str()
        .unwrap_or_else(|| panic!("printer id"))
        .to_owned()
}

async fn code(f: &Fixture, id: &str) -> String {
    let response = admin(
        f,
        Method::POST,
        &format!("/api/v2/print/printers/{id}/pairing-code"),
        json!(null),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    let code = value(&response)["code"]
        .as_str()
        .unwrap_or_else(|| panic!("pairing code"))
        .to_owned();
    assert_eq!(code.len(), 9);
    assert_eq!(&code[4..5], "-");
    assert!(
        code.bytes()
            .filter(|byte| *byte != b'-')
            .all(|byte| b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ".contains(&byte))
    );
    assert!(value(&response)["expires_at_unix_ms"].as_i64().is_some());
    code
}

async fn pair(f: &Fixture, body: Value, source: u8) -> Captured {
    let request = must(
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/print/client/pair")
            .extension(ConnectInfo(PeerAddress(IpAddr::from([192, 0, 2, source]))))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
    );
    let response = must(drive(&f.router, request).await);
    assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    response
}

async fn confirm(f: &Fixture, token: &str) -> Captured {
    let request = must(
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/print/client/confirmation")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(unread_body()),
    );
    let response = must(drive(&f.router, request).await);
    assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    response
}

#[tokio::test]
async fn pairing_is_one_time_then_confirmed_and_replacement_requires_explicit_cleanup() {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = must(Fixture::new().await);
    let id = create(&f).await;
    let old = code(&f, &id).await;
    let new = code(&f, &id).await;
    assert_eq!(
        pair(&f, json!({"protocol_version":1,"code":old}), 1)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    let paired = pair(
        &f,
        json!({"protocol_version":1,"code":new.to_ascii_lowercase()}),
        1,
    )
    .await;
    assert_eq!(paired.status, StatusCode::OK);
    let issued = value(&paired);
    assert_eq!(issued["protocol_version"], 1);
    assert_eq!(issued["printer_id"], id);
    let token = issued["token"]
        .as_str()
        .unwrap_or_else(|| panic!("client token"));
    assert!(token.starts_with("npc1."));
    assert_eq!(
        pair(&f, json!({"protocol_version":1,"code":new}), 1)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    let pending = admin(&f, Method::GET, "/api/v2/print/printers", json!(null)).await;
    assert_eq!(value(&pending)[0]["client_confirmed"], false);
    assert_eq!(value(&pending)[0]["client_id"], issued["client_id"]);
    assert_eq!(
        value(&pending)[0].as_object().map(serde_json::Map::len),
        Some(5)
    );
    assert!(!String::from_utf8_lossy(&pending.body).contains(token));
    let refused = admin(
        &f,
        Method::POST,
        &format!("/api/v2/print/printers/{id}/pairing-code"),
        json!(null),
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    let confirmed = confirm(&f, token).await;
    assert_eq!(confirmed.status, StatusCode::OK);
    assert_eq!(value(&confirmed)["client_confirmed"], true);
    assert_eq!(value(&confirm(&f, token).await), value(&confirmed));
    let reset = admin(
        &f,
        Method::DELETE,
        &format!("/api/v2/print/printers/{id}/binding"),
        json!(null),
    )
    .await;
    assert_eq!(reset.status, StatusCode::NO_CONTENT);
    assert_eq!(confirm(&f, token).await.status, StatusCode::UNAUTHORIZED);
    let next = code(&f, &id).await;
    let paired = pair(&f, json!({"protocol_version":1,"code":next}), 1).await;
    assert_eq!(paired.status, StatusCode::OK);
    assert_ne!(value(&paired)["client_id"], issued["client_id"]);
}

#[tokio::test]
async fn printer_management_rejects_unauthenticated_and_viewer_requests_before_reading_bodies() {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = must(Fixture::new().await);
    let id = create(&f).await;
    for cookie in [None, Some(f.cookie.as_str())] {
        if cookie.is_some() {
            must(f.make_viewer().await);
        }
        for (method, path) in [
            (Method::GET, "/api/v2/print/printers".to_owned()),
            (Method::POST, "/api/v2/print/printers".to_owned()),
            (
                Method::POST,
                format!("/api/v2/print/printers/{id}/pairing-code"),
            ),
            (
                Method::DELETE,
                format!("/api/v2/print/printers/{id}/binding"),
            ),
        ] {
            let mut request = Request::builder().method(method).uri(path);
            if let Some(cookie) = cookie {
                request = request.header(header::COOKIE, cookie);
            }
            let response = must(drive(&f.router, must(request.body(unread_body()))).await);
            assert_eq!(
                response.status,
                if cookie.is_some() {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::UNAUTHORIZED
                }
            );
            assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
        }
    }
}

#[tokio::test]
async fn pairing_limits_trust_socket_ip_and_reject_before_body_reads() {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = must(Fixture::new().await);
    for attempt in 1..=5 {
        let request = must(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v2/print/client/pair")
                .extension(ConnectInfo(PeerAddress(IpAddr::from([192, 0, 2, 1]))))
                .header("x-forwarded-for", format!("203.0.113.{attempt}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{")),
        );
        assert_eq!(
            must(drive(&f.router, request).await).status,
            StatusCode::BAD_REQUEST
        );
    }
    let request = must(
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/print/client/pair")
            .extension(ConnectInfo(PeerAddress(IpAddr::from([192, 0, 2, 1]))))
            .body(unread_body()),
    );
    let limited = must(drive(&f.router, request).await);
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers[header::RETRY_AFTER], "60");
    assert_eq!(limited.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(value(&limited)["code"], "RATE_LIMITED");
    assert_eq!(
        pair(&f, json!({"protocol_version":1,"code":"ABCD2345"}), 2)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    let missing_peer = must(
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/print/client/pair")
            .header("x-forwarded-for", "192.0.2.3")
            .body(unread_body()),
    );
    assert_eq!(
        must(drive(&f.router, missing_peer).await).status,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn client_credentials_are_isolated_from_sources_and_operator_sessions() {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = must(Fixture::new().await);
    let id = create(&f).await;
    let code = code(&f, &id).await;
    let paired = pair(&f, json!({"protocol_version":1,"code":code}), 1).await;
    let issued = value(&paired);
    let token = issued["token"]
        .as_str()
        .unwrap_or_else(|| panic!("client token"));
    for path in [
        "/api/v2/print/sources",
        "/api/v2/print/printers",
        "/api/v2/print/queue",
    ] {
        let request = must(
            Request::builder()
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(unread_body()),
        );
        assert_eq!(
            must(drive(&f.router, request).await).status,
            StatusCode::UNAUTHORIZED
        );
    }
    let request = must(
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/print/requests")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(unread_body()),
    );
    assert_eq!(
        must(drive(&f.router, request).await).status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(confirm(&f, &f.token).await.status, StatusCode::UNAUTHORIZED);
    let request = must(
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/print/client/confirmation")
            .header(header::COOKIE, &f.cookie)
            .body(unread_body()),
    );
    assert_eq!(
        must(drive(&f.router, request).await).status,
        StatusCode::UNAUTHORIZED
    );
    let mut headers = HeaderMap::new();
    headers.append(
        header::AUTHORIZATION,
        must(format!("Bearer {token}").parse()),
    );
    headers.append(
        header::AUTHORIZATION,
        must(format!("Bearer {token}").parse()),
    );
    let mut request = must(
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/print/client/confirmation")
            .body(unread_body()),
    );
    *request.headers_mut() = headers;
    assert_eq!(
        must(drive(&f.router, request).await).status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn pairing_codes_and_client_tokens_do_not_escape_into_trace_logs() {
    use crate::{
        config::LogLevel,
        logging::tests::{CapturedLogs, SubscriberTestGuard},
    };
    use sha2::{Digest as _, Sha256};
    use tracing::instrument::WithSubscriber as _;

    let _subscriber = SubscriberTestGuard::acquire();
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = must(Fixture::new().await);
    let captured = CapturedLogs::default();
    let subscriber = captured.subscriber(LogLevel::Trace);
    let (code, token) = async {
        let id = create(&f).await;
        let code = code(&f, &id).await;
        let paired = pair(&f, json!({"protocol_version":1,"code":code}), 1).await;
        assert_eq!(paired.status, StatusCode::OK);
        let token = value(&paired)["token"]
            .as_str()
            .unwrap_or_else(|| panic!("client token"))
            .to_owned();
        assert_eq!(confirm(&f, &token).await.status, StatusCode::OK);
        assert_eq!(
            pair(&f, json!({"protocol_version":1,"code":code}), 1)
                .await
                .status,
            StatusCode::UNAUTHORIZED
        );
        (code, token)
    }
    .with_subscriber(subscriber)
    .await;
    let text = must(captured.text());
    assert!(text.contains("print_authentication"));
    for secret in [
        &code,
        &code.replace('-', ""),
        &token,
        &hex::encode(Sha256::digest(token.as_bytes())),
    ] {
        assert!(!text.contains(secret));
    }
}

#[tokio::test]
async fn printer_and_pairing_inputs_are_closed_and_generic() {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = must(Fixture::new().await);
    for name in [String::new(), "a".repeat(129), "printer\nname".to_owned()] {
        assert_eq!(
            admin(
                &f,
                Method::POST,
                "/api/v2/print/printers",
                json!({"name":name})
            )
            .await
            .status,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        admin(
            &f,
            Method::POST,
            "/api/v2/print/printers",
            json!({"name":"printer","client_id":"extra"})
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    for (index, body) in [
        json!({"protocol_version":2,"code":"ABCD2345"}),
        json!({"protocol_version":1,"code":"ABCD2345","printer_id":"extra"}),
        json!({"protocol_version":1}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            pair(&f, body, u8::try_from(index + 10).unwrap_or_default())
                .await
                .status,
            StatusCode::BAD_REQUEST
        );
    }
    let unknown = pair(&f, json!({"protocol_version":1,"code":"ABCD2345"}), 20).await;
    for (index, code) in ["", "éé-2345", "ABCD-2345-extra", "00000000"]
        .into_iter()
        .enumerate()
    {
        let invalid = pair(
            &f,
            json!({"protocol_version":1,"code":code}),
            u8::try_from(index + 21).unwrap_or_default(),
        )
        .await;
        assert_eq!(invalid.status, StatusCode::UNAUTHORIZED);
        assert_eq!(value(&invalid), value(&unknown));
    }
    assert_eq!(
        admin(
            &f,
            Method::POST,
            "/api/v2/print/printers/not-an-id/pairing-code",
            json!(null)
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        admin(
            &f,
            Method::POST,
            &format!(
                "/api/v2/print/printers/{}/pairing-code",
                uuid::Uuid::now_v7()
            ),
            json!(null)
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}
