use std::{
    fmt::Write as _,
    io::{Cursor, Write as _},
};

use axum::{
    body::{Body, Bytes, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use super::{
    LOGIN_NAME, PASSWORD, PasswordVerificationTestGuard, TestDatabase, drive, header_text,
    login_request, router, seed_operator, server_state, unused_web_root,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn streamed_body(bytes: Vec<u8>) -> Body {
    let bytes = Bytes::from(bytes);
    let chunks: Vec<_> = (0..bytes.len())
        .step_by(4096)
        .map(|start| bytes.slice(start..bytes.len().min(start + 4096)))
        .collect();
    Body::from_stream(futures_util::stream::iter(
        chunks.into_iter().map(Ok::<_, std::io::Error>),
    ))
}

#[tokio::test]
async fn json_extraction_rejections_remain_bad_requests() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(&fixture.database, LOGIN_NAME, PASSWORD).await?;
    let application = router(server_state(&fixture)?, unused_web_root());
    let login = drive(&application, login_request(LOGIN_NAME, PASSWORD)?).await?;
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or("missing session cookie")?;
    for (method, path, authenticated) in [
        (
            Method::PATCH,
            "/api/v2/devices/01900000-0000-7000-8000-000000000001",
            true,
        ),
        (Method::PUT, "/api/v2/provisioning-window", true),
        (Method::POST, "/api/v2/target-submissions", true),
        (
            Method::PATCH,
            "/api/v2/operator/accounts/01900000-0000-7000-8000-000000000001",
            true,
        ),
        (Method::POST, "/api/v2/operator/invitations", true),
        (Method::POST, "/api/v2/operator/register/inspect", false),
        (Method::POST, "/api/v2/operator/register", false),
        (
            Method::POST,
            "/api/v2/operator/password/reset/inspect",
            false,
        ),
        (Method::POST, "/api/v2/operator/password/reset", false),
        (Method::POST, "/api/v2/operator/password/change", true),
    ] {
        for (content_type, body) in [
            ("application/json", "{"),
            ("application/json", "{}"),
            ("text/plain", "{}"),
        ] {
            let mut builder = Request::builder()
                .method(method.clone())
                .uri(path)
                .header(header::CONTENT_TYPE, content_type);
            if authenticated {
                builder = builder.header(header::COOKIE, cookie);
            }
            let response = drive(&application, builder.body(Body::from(body))?).await?;
            assert_eq!(response.status, StatusCode::BAD_REQUEST, "{method} {path}");
            let error: Value = serde_json::from_slice(&response.body)?;
            assert_eq!(error["code"], "INVALID_REQUEST");
        }
    }
    let unread = Body::from_stream(futures_util::stream::poll_fn(
        |_| -> std::task::Poll<Option<Result<Bytes, std::io::Error>>> {
            panic!("an invalid operator ID must be rejected before reading the body")
        },
    ));
    let response = drive(
        &application,
        Request::builder()
            .method(Method::PATCH)
            .uri("/api/v2/operator/accounts/not-an-id")
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(unread)?,
    )
    .await?;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn json_routes_use_default_body_limit_for_declared_and_streamed_requests() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(&fixture.database, LOGIN_NAME, PASSWORD).await?;
    let application = router(server_state(&fixture)?, unused_web_root());
    let login = drive(&application, login_request(LOGIN_NAME, PASSWORD)?).await?;
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or("missing session cookie")?;
    let invitation = drive(
        &application,
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/operator/invitations")
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"role":"viewer"}).to_string()))?,
    )
    .await?;
    assert_eq!(invitation.status, StatusCode::CREATED);
    let invitation: Value = serde_json::from_slice(&invitation.body)?;
    for (method, path, payload, status, authenticated) in [
        (
            Method::POST,
            "/api/v2/operator/register/inspect",
            json!({"token":invitation["token"]}),
            StatusCode::OK,
            false,
        ),
        (
            Method::PATCH,
            "/api/v2/devices/01900000-0000-7000-8000-000000000001",
            json!({"state":"enabled"}),
            StatusCode::NOT_FOUND,
            true,
        ),
        (
            Method::PUT,
            "/api/v2/provisioning-window",
            json!({"state":"closed"}),
            StatusCode::OK,
            true,
        ),
        (
            Method::POST,
            "/api/v2/target-submissions",
            json!({"operation_id":"01900000-0000-7000-8000-000000001000", "scope":{"kind":"all_enabled"}, "action":{"kind":"reset_home"}}),
            StatusCode::OK,
            true,
        ),
        (
            Method::POST,
            "/api/v2/session",
            json!({"login_name":LOGIN_NAME,"password":PASSWORD}),
            StatusCode::OK,
            false,
        ),
    ] {
        for (declared, excess) in [(true, 0), (false, 0), (true, 1), (false, 1)] {
            let mut bytes = serde_json::to_vec(&payload)?;
            bytes.resize(2 * 1024 * 1024 + excess, b' ');
            let mut builder = Request::builder()
                .method(method.clone())
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json");
            if authenticated {
                builder = builder.header(header::COOKIE, cookie);
            }
            let body = if declared {
                builder = builder.header(header::CONTENT_LENGTH, bytes.len());
                Body::from(bytes)
            } else {
                streamed_body(bytes)
            };
            let response = drive(&application, builder.body(body)?).await?;
            let expected = if excess == 0 {
                status
            } else {
                StatusCode::BAD_REQUEST
            };
            assert_eq!(
                response.status, expected,
                "{method} {path}, declared: {declared}, excess: {excess}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn fully_escaped_six_hundred_device_submission_is_accepted() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(&fixture.database, LOGIN_NAME, PASSWORD).await?;
    let application = router(server_state(&fixture)?, unused_web_root());
    let login = drive(&application, login_request(LOGIN_NAME, PASSWORD)?).await?;
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or("missing session cookie")?;
    let mut ids = Vec::new();
    for index in 0..600 {
        let id = format!("01900000-0000-7000-8000-{index:012x}");
        let mut escaped = String::new();
        for byte in id.bytes() {
            write!(escaped, "\\u{byte:04x}")?;
        }
        ids.push(format!("\"{escaped}\""));
    }
    let ids = ids.join(",");
    let submission = format!(
        "{{\"operation_id\":\"01900000-0000-7000-8000-000000001000\",\"scope\":{{\"kind\":\"devices\",\"device_ids\":[{ids}]}},\"action\":{{\"kind\":\"reset_home\"}}}}"
    );
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v2/target-submissions")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(streamed_body(submission.into_bytes()))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 256 * 1024).await?;
    let response: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(response["results"].as_array().map(Vec::len), Some(600));
    Ok(())
}

fn padded_workbook(padding: usize) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let mut source = ZipArchive::new(Cursor::new(include_bytes!(
        "../../component/import/examples/roster.xlsx"
    )))?;
    let mut output = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..source.len() {
        output.raw_copy_file(source.by_index(index)?)?;
    }
    output.start_file(
        "budget-padding.bin",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    output.write_all(&vec![0; padding])?;
    Ok(output.finish()?.into_inner())
}

#[tokio::test]
async fn workbook_uploads_use_default_two_mib_limit() -> TestResult {
    const LIMIT: usize = 2 * 1024 * 1024;
    const CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(&fixture.database, LOGIN_NAME, PASSWORD).await?;
    let application = router(server_state(&fixture)?, unused_web_root());
    let login = drive(&application, login_request(LOGIN_NAME, PASSWORD)?).await?;
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or("missing session cookie")?;
    for path in [
        "/api/v2/imports",
        "/api/v2/imports/01900000-0000-7000-8000-000000000001/actions/commit",
    ] {
        for declared in [true, false] {
            let bytes = vec![0; LIMIT + 1];
            let mut builder = Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, CONTENT_TYPE);
            if path != "/api/v2/imports" {
                builder = builder.header("x-natsume-preview-token", "A".repeat(43));
            }
            let body = if declared {
                builder = builder.header(header::CONTENT_LENGTH, bytes.len());
                Body::from(bytes)
            } else {
                streamed_body(bytes)
            };
            let response = drive(&application, builder.body(body)?).await?;
            assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE, "{path}");
        }
    }
    let workbook = padded_workbook(LIMIT - padded_workbook(0)?.len())?;
    assert_eq!(workbook.len(), LIMIT);
    let preview = drive(
        &application,
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/imports")
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, CONTENT_TYPE)
            .body(streamed_body(workbook.clone()))?,
    )
    .await?;
    assert_eq!(preview.status, StatusCode::CREATED);
    let preview: Value = serde_json::from_slice(&preview.body)?;
    let candidate = preview["candidate_id"]
        .as_str()
        .ok_or("missing candidate")?;
    let token = preview["preview_token"].as_str().ok_or("missing token")?;
    let committed = drive(
        &application,
        Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v2/imports/{candidate}/actions/commit"))
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, CONTENT_TYPE)
            .header("x-natsume-preview-token", token)
            .body(streamed_body(workbook))?,
    )
    .await?;
    assert_eq!(committed.status, StatusCode::NO_CONTENT);
    Ok(())
}
