use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use diesel::{ExpressionMethods as _, RunQueryDsl as _};
use serde_json::Value;

use super::{
    ExecutionTestError, Fixture, PasswordVerificationTestGuard, TestResult, drive, paired, ready,
    unread_body,
};
use crate::{component::print::PrintError, diesel_schema::print_jobs};

async fn exhausted() -> Result<Fixture, ExecutionTestError> {
    let f = Fixture::new()
        .await
        .map_err(|_| ExecutionTestError::Setup)?;
    ready(&f).await?;
    f.database
        .database
        .write(|tx| {
            diesel::update(print_jobs::table)
                .set((
                    print_jobs::state.eq("exhausted"),
                    print_jobs::attempts_used.eq(3),
                ))
                .execute(tx.connection())?;
            Ok::<_, PrintError>(())
        })
        .await
        .map_err(|_| ExecutionTestError::Setup)?;
    Ok(f)
}

async fn requeue(
    f: &Fixture,
    path: &str,
    body: &str,
) -> Result<super::Captured, ExecutionTestError> {
    Ok(drive(
        &f.router,
        Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::COOKIE, &f.cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))?,
    )
    .await?)
}

#[tokio::test]
async fn admin_requeue_changes_one_round_and_rejects_duplicate_or_stale_requests() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = exhausted().await?;
    let path = "/api/v2/print/executions/1/requeue";
    let response = requeue(&f, path, r#"{"expected_requeue_count":0}"#).await?;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    assert!(response.body.is_empty());
    assert_eq!(
        requeue(&f, path, r#"{"expected_requeue_count":0}"#)
            .await?
            .status,
        StatusCode::CONFLICT
    );
    f.database
        .database
        .read(|tx| {
            use diesel::QueryDsl as _;
            let job = print_jobs::table
                .select((
                    print_jobs::state,
                    print_jobs::attempts_used,
                    print_jobs::requeue_count,
                ))
                .first::<(String, i32, i32)>(tx.connection())?;
            assert_eq!(job, ("queued".to_owned(), 0, 1));
            Ok::<_, PrintError>(())
        })
        .await
        .map_err(|_| ExecutionTestError::Setup)?;
    f.database
        .database
        .write(|tx| {
            diesel::update(print_jobs::table)
                .set((
                    print_jobs::state.eq("exhausted"),
                    print_jobs::attempts_used.eq(3),
                ))
                .execute(tx.connection())?;
            Ok::<_, PrintError>(())
        })
        .await
        .map_err(|_| ExecutionTestError::Setup)?;
    assert_eq!(
        requeue(&f, path, r#"{"expected_requeue_count":0}"#)
            .await?
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        requeue(&f, path, r#"{"expected_requeue_count":1}"#)
            .await?
            .status,
        StatusCode::NO_CONTENT
    );
    Ok(())
}

#[tokio::test]
async fn requeue_rejects_invalid_inputs_and_missing_documents_with_the_public_envelope()
-> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = exhausted().await?;
    for suffix in ["invalid", "0", "-1", "9223372036854775808"] {
        let result = requeue(
            &f,
            &format!("/api/v2/print/executions/{suffix}/requeue"),
            r#"{"expected_requeue_count":0}"#,
        )
        .await?;
        assert_eq!(result.status, StatusCode::BAD_REQUEST);
        let body: Value = serde_json::from_slice(&result.body)?;
        assert_eq!(body["code"], "INVALID_REQUEST");
    }
    for body in [
        "{}",
        "invalid",
        r#"{"expected_requeue_count":-1}"#,
        r#"{"expected_requeue_count":0,"extra":true}"#,
        r#"{"expected_requeue_count":0.5}"#,
    ] {
        let result = requeue(&f, "/api/v2/print/executions/1/requeue", body).await?;
        assert_eq!(result.status, StatusCode::BAD_REQUEST);
        assert_eq!(result.headers[header::CACHE_CONTROL], "no-store");
    }
    assert_eq!(
        requeue(
            &f,
            "/api/v2/print/executions/999/requeue",
            r#"{"expected_requeue_count":0}"#
        )
        .await?
        .status,
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn requeue_requires_admin_before_reading_the_request_body() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = exhausted().await?;
    let client = paired(&f, true).await?;
    let client_token = client["token"].as_str().ok_or(ExecutionTestError::Setup)?;
    for token in [None, Some(f.token.as_str()), Some(client_token)] {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri("/api/v2/print/executions/1/requeue");
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = drive(&f.router, request.body(unread_body())?).await?;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    }
    f.make_viewer()
        .await
        .map_err(|_| ExecutionTestError::Setup)?;
    let response = drive(
        &f.router,
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/print/executions/1/requeue")
            .header(header::COOKIE, &f.cookie)
            .body(unread_body())?,
    )
    .await?;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        serde_json::from_slice::<Value>(&response.body)?["code"],
        "AUTHORIZATION_DENIED"
    );
    Ok(())
}
