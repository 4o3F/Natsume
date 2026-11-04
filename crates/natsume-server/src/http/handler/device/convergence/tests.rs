use axum::http::{Method, StatusCode, header};

use crate::{
    component::operator::tests::PasswordVerificationTestGuard,
    http::tests::{
        TestDatabase, cookie_request, drive, header_text, login_request, seed_operator, server,
        unused_web_root,
    },
    test_error::TestError,
};

#[tokio::test]
async fn invalid_and_missing_devices_preserve_convergence_error_responses() -> Result<(), TestError>
{
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(
        &fixture.database,
        "convergence-admin",
        "convergence-test-password1!",
    )
    .await?;
    let loaded = server(&fixture)?;
    let application = loaded.http_router(unused_web_root());
    let login = drive(
        &application,
        login_request("convergence-admin", "convergence-test-password1!")?,
    )
    .await?;
    assert_eq!(login.status, StatusCode::OK);
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or(TestError::missing("session cookie"))?;

    for (device_id, status, title, code) in [
        (
            "invalid-device-id",
            StatusCode::BAD_REQUEST,
            "Bad Request",
            "INVALID_REQUEST",
        ),
        (
            "01900000-0000-7000-8000-000000000001",
            StatusCode::NOT_FOUND,
            "Not Found",
            "RESOURCE_NOT_FOUND",
        ),
    ] {
        let response = drive(
            &application,
            cookie_request(
                Method::GET,
                &format!("/api/v2/devices/{device_id}/convergence"),
                cookie,
            )?,
        )
        .await?;
        assert_eq!(response.status, status, "device ID: {device_id}");
        assert_eq!(
            header_text(&response.headers, &header::CONTENT_TYPE)?,
            "application/json",
            "device ID: {device_id}"
        );
        let body: serde_json::Value = serde_json::from_slice(&response.body)?;
        assert_eq!(
            body,
            serde_json::json!({ "title": title, "status": status.as_u16(), "code": code }),
            "device ID: {device_id}"
        );
    }
    Ok(())
}
