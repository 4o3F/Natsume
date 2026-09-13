use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use diesel::connection::SimpleConnection;
use serde_json::{Value, json};

use super::TargetSubmissionBody;
use crate::{
    component::operator::tests::PasswordVerificationTestGuard,
    db::PersistenceError,
    http::{
        router,
        tests::{
            TestDatabase, drive, header_text, login_request, request, seed_operator, server_state,
            unused_web_root,
        },
    },
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn body() -> Value {
    json!({"operation_id":"ec7737b0-1b2c-4def-8123-0123456789ab", "scope":{"kind":"all_enabled"}, "action":{"kind":"reset_home"}})
}

#[test]
fn submissions_require_closed_actions_scopes_and_canonical_ids() {
    let valid = body();
    let mut invalid = vec![json!({}), json!({"foreground_target":"contest"})];
    for operation_id in [
        "bad",
        "EC7737B0-1B2C-4DEF-8123-0123456789AB",
        "00000000-0000-0000-0000-000000000000",
    ] {
        let mut input = valid.clone();
        input["operation_id"] = json!(operation_id);
        invalid.push(input);
    }
    for scope in [
        json!({"kind":"devices","device_ids":[]}),
        json!({"kind":"devices","device_ids":["bad"]}),
        json!({"kind":"all_enabled", "device_ids":[]}),
        json!({"kind":"unknown"}),
    ] {
        let mut input = valid.clone();
        input["scope"] = scope;
        invalid.push(input);
    }
    for action in [
        json!({"kind":"reset_home","unexpected":true}),
        json!({"kind":"set_foreground","foreground_target":"greeter"}),
        json!({"kind":"set_foreground"}),
        json!({"kind":"unknown"}),
    ] {
        let mut input = valid.clone();
        input["action"] = action;
        invalid.push(input);
    }
    for input in invalid {
        assert!(
            serde_json::from_value::<TargetSubmissionBody>(input.clone())
                .ok()
                .and_then(TargetSubmissionBody::into_request)
                .is_none(),
            "accepted {input}"
        );
    }
    for action in [
        json!({"kind":"set_foreground","foreground_target":"contest"}),
        json!({"kind":"set_foreground","foreground_target":"waiting"}),
        json!({"kind":"terminate_session"}),
        json!({"kind":"reset_home"}),
    ] {
        let mut input = valid.clone();
        input["action"] = action;
        assert!(
            serde_json::from_value::<TargetSubmissionBody>(input)
                .ok()
                .and_then(TargetSubmissionBody::into_request)
                .is_some()
        );
    }
}

#[tokio::test]
async fn batch_endpoint_is_admin_only_replayable_and_replaces_old_write_routes() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    seed_operator(&fixture.database, "batch-admin", "batch-test-password").await?;
    fixture.database.write(|tx| {
        tx.connection().batch_execute("INSERT INTO devices VALUES ('01900000-0000-7000-8000-000000000001','m1','strong','enabled',1), ('01900000-0000-7000-8000-000000000002','m2','strong','disabled',1);").map_err(|_| PersistenceError::OperationFailed)
    }).await.map_err(|e| format!("seed: {e:?}"))?;
    let application = router(server_state(fixture.database.clone())?, unused_web_root());
    let path = "/api/v2/target-submissions";
    assert_eq!(
        drive(&application, request(Method::POST, path, "")?)
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    let login = drive(
        &application,
        login_request("batch-admin", "batch-test-password")?,
    )
    .await?;
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or("cookie absent")?
        .to_owned();
    let make_request = |payload: &Value| {
        Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::COOKIE, &cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(payload.to_string()))
    };
    let first = drive(&application, make_request(&body())?).await?;
    assert_eq!(first.status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&first.body)?;
    assert_eq!(
        value["results"],
        json!([{"device_id":"01900000-0000-7000-8000-000000000001","status":"submitted"}])
    );
    fixture
        .database
        .write(|tx| {
            tx.connection()
                .batch_execute("UPDATE devices SET state='disabled';")
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(|e| format!("state: {e:?}"))?;
    let replay = drive(&application, make_request(&body())?).await?;
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(replay.body, first.body);
    let mut changed = body();
    changed["action"] = json!({"kind":"terminate_session"});
    assert_eq!(
        drive(&application, make_request(&changed)?).await?.status,
        StatusCode::CONFLICT
    );
    changed["operation_id"] = json!("be7737b0-1b2c-4def-8123-0123456789ab");
    changed["scope"] = json!({"kind":"devices","device_ids":["01900000-0000-7000-8000-000000000001", "01900000-0000-7000-8000-000000000099"]});
    let rejected = drive(&application, make_request(&changed)?).await?;
    assert_eq!(rejected.status, StatusCode::OK);
    let rejected: Value = serde_json::from_slice(&rejected.body)?;
    assert_eq!(rejected["results"][0]["code"], "device_not_enabled");
    assert_eq!(rejected["results"][1]["code"], "device_not_found");
    fixture
        .database
        .write(|tx| {
            tx.connection()
                .batch_execute("UPDATE operator_accounts SET role='viewer';")
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(|e| format!("role: {e:?}"))?;
    assert_eq!(
        drive(&application, make_request(&body())?).await?.status,
        StatusCode::FORBIDDEN
    );
    Ok(())
}

#[tokio::test]
async fn old_target_write_routes_are_removed() -> TestResult {
    let fixture = TestDatabase::new().await?;
    let application = router(server_state(fixture.database.clone())?, unused_web_root());
    for (method, old_path, status) in [
        (
            Method::PUT,
            "/api/v2/devices/01900000-0000-7000-8000-000000000001/session-control",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            Method::POST,
            "/api/v2/devices/01900000-0000-7000-8000-000000000001/session-control/actions/terminate",
            StatusCode::NOT_FOUND,
        ),
        (
            Method::POST,
            "/api/v2/devices/01900000-0000-7000-8000-000000000001/home/actions/reset",
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            drive(&application, request(method, old_path, "")?)
                .await?
                .status,
            status
        );
    }
    Ok(())
}
