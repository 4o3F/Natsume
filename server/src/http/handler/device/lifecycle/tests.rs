use axum::http::{Method, StatusCode, header};
use diesel::connection::SimpleConnection;
use serde_json::Value;

use crate::{
    component::operator::tests::PasswordVerificationTestGuard,
    db::PersistenceError,
    http::{
        router,
        tests::{
            TestDatabase, cookie_request, drive, header_text, login_request, seed_operator,
            server_state, unused_web_root,
        },
    },
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn device_state_query_filters_at_the_server_and_preserves_records() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(
        &fixture.database,
        "list-admin",
        "device-filter-test-password",
    )
    .await?;
    fixture.database.write(|tx| tx.connection().batch_execute(
        "INSERT INTO devices VALUES
        ('01900000-0000-7000-8000-000000000001','11111111-1111-5111-8111-111111111111','strong','enabled',1),
        ('01900000-0000-7000-8000-000000000002','22222222-2222-5222-8222-222222222222','strong','disabled',1),
        ('01900000-0000-7000-8000-000000000003','33333333-3333-5333-8333-333333333333','strong','revoked',1);"
    ).map_err(|_| PersistenceError::OperationFailed)).await.map_err(|error| format!("seed: {error:?}"))?;
    let application = router(server_state(fixture.database.clone())?, unused_web_root());
    let login = drive(
        &application,
        login_request("list-admin", "device-filter-test-password")?,
    )
    .await?;
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or("missing cookie")?;
    for (query, expected) in [
        ("?state=enabled", vec!["enabled"]),
        ("?state=disabled", vec!["disabled"]),
        ("?state=revoked", vec!["revoked"]),
        ("?state=non_revoked", vec!["enabled", "disabled"]),
        ("?state=all", vec!["enabled", "disabled", "revoked"]),
        ("", vec!["enabled", "disabled", "revoked"]),
    ] {
        let response = drive(
            &application,
            cookie_request(Method::GET, &format!("/api/v2/devices{query}"), cookie)?,
        )
        .await?;
        assert_eq!(response.status, StatusCode::OK);
        let rows: Vec<Value> = serde_json::from_slice(&response.body)?;
        assert_eq!(
            rows.iter()
                .filter_map(|row| row["state"].as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }
    for query in [
        "?state=unknown",
        "?state=",
        "?state=Enabled",
        "?state=enabled&state=revoked",
        "?filter=enabled",
    ] {
        let response = drive(
            &application,
            cookie_request(Method::GET, &format!("/api/v2/devices{query}"), cookie)?,
        )
        .await?;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        let error: Value = serde_json::from_slice(&response.body)?;
        assert_eq!(error["code"], "INVALID_REQUEST");
    }
    // Filtering precedes projection parsing, so an excluded revoked identity
    // cannot break the enabled view. Viewers have the same read filter contract.
    fixture.database.write(|tx| tx.connection().batch_execute(
        "UPDATE devices SET machine_hardware_id='invalid' WHERE state='revoked'; UPDATE operator_accounts SET role='viewer';"
    ).map_err(|_| PersistenceError::OperationFailed)).await.map_err(|error| format!("fixture update: {error:?}"))?;
    let response = drive(
        &application,
        cookie_request(Method::GET, "/api/v2/devices?state=enabled", cookie)?,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK);
    let rows: Vec<Value> = serde_json::from_slice(&response.body)?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["state"], "enabled");
    Ok(())
}
