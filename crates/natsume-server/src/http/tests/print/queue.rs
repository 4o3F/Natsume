use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use diesel::{ExpressionMethods, RunQueryDsl};
use serde_json::{Value, json};
use snafu::Snafu;

use super::{Fixture, PasswordVerificationTestGuard, drive};
use crate::{
    component::print::PrintError,
    diesel_schema::{accounts, print_documents, print_requests, print_sources},
    http::tests::{Captured, SupportFailure},
};

#[derive(Debug, Snafu)]
enum QueueTestError {
    #[snafu(context(false), display("test infrastructure operation failed"))]
    Source {
        #[snafu(source(from(crate::test_error::TestError, Box::new)))]
        source: Box<crate::test_error::TestError>,
    },
    #[snafu(display("print queue fixture setup failed"))]
    Setup,
    #[snafu(context(false), display("queue test request failed"))]
    Request { source: axum::http::Error },
    #[snafu(context(false), display("queue test response failed"))]
    Response { source: SupportFailure },
    #[snafu(context(false), display("queue response is not JSON"))]
    Json { source: serde_json::Error },
    #[snafu(context(false), display("queue test filesystem operation failed"))]
    Io { source: std::io::Error },
}

type TestResult = Result<(), QueueTestError>;

async fn read_queue(f: &Fixture) -> Result<Captured, QueueTestError> {
    Ok(drive(
        &f.router,
        Request::builder()
            .uri("/api/v2/print/queue")
            .header(header::COOKIE, &f.cookie)
            .body(Body::empty())?,
    )
    .await?)
}

#[tokio::test]
async fn queue_requires_operator_session_and_allows_both_roles() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await.map_err(QueueTestError::from)?;
    for bearer in [false, true] {
        let mut request = Request::builder().uri("/api/v2/print/queue");
        if bearer {
            request = request.header(header::AUTHORIZATION, format!("Bearer {}", f.token));
        }
        let response = drive(&f.router, request.body(Body::empty())?).await?;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    }
    for viewer in [false, true] {
        if viewer {
            f.make_viewer().await.map_err(|source| {
                QueueTestError::from(crate::test_error::TestError::from(source))
            })?;
        }
        let response = read_queue(&f).await?;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(serde_json::from_slice::<Value>(&response.body)?, json!([]));
    }
    for method in [Method::POST, Method::PUT, Method::DELETE, Method::HEAD] {
        let expected = if method == Method::HEAD {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::METHOD_NOT_ALLOWED
        };
        let response = drive(
            &f.router,
            Request::builder()
                .method(method)
                .uri("/api/v2/print/queue")
                .header(header::COOKIE, &f.cookie)
                .body(Body::empty())?,
        )
        .await?;
        assert_eq!(response.status, expected);
        assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    }
    Ok(())
}

#[tokio::test]
async fn queue_follows_admission_order_without_duplicating_replays() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await.map_err(QueueTestError::from)?;
    let first = f.register().await.map_err(QueueTestError::from)?;
    let second = f.register().await.map_err(QueueTestError::from)?;
    assert_eq!(
        serde_json::from_slice::<Value>(&read_queue(&f).await?.body)?,
        json!([])
    );
    for id in [&second, &second, &first] {
        let response = f
            .send(
                Method::PUT,
                &format!("/api/v2/print/requests/{id}/document"),
                "application/pdf",
                Body::from(super::pdf(2)),
            )
            .await
            .map_err(QueueTestError::from)?;
        assert_eq!(response.status, StatusCode::OK);
    }
    let response = read_queue(&f).await?;
    assert_eq!(response.status, StatusCode::OK);
    let queue: Vec<Value> = serde_json::from_slice(&response.body)?;
    assert_eq!(queue.len(), 2);
    for (entry, (id, sequence)) in queue.iter().zip([(&second, 1), (&first, 2)]) {
        assert_eq!(entry["request_id"], *id);
        assert_eq!(entry["source_id"], f.source_id);
        assert_eq!(entry["ready_sequence"], sequence);
        assert_eq!(entry["pages"], 2);
        assert_eq!(entry["team_name"], "队伍");
    }
    Ok(())
}

#[tokio::test]
async fn queue_preserves_cross_source_fifo_and_snapshots_using_only_metadata() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await.map_err(QueueTestError::from)?;
    let source = f.source_id.clone();
    f.database
        .database
        .write(move |transaction| {
            diesel::insert_into(print_sources::table)
                .values((
                    print_sources::source_id.eq("second-source"),
                    print_sources::name.eq("Second judge"),
                    print_sources::token_hash.eq(&[1_u8][..]),
                    print_sources::revoked.eq(1),
                ))
                .execute(transaction.connection())?;
            let original_snapshot = ("原队伍", Some("A-01"), "main.cpp");
            let other_snapshot = ("Other team", None, "other.cpp");
            for (source_id, request_id, (team_name, seat_label, filename), failure) in [
                (source.as_str(), "shared-request", original_snapshot, None),
                (source.as_str(), "preparing", original_snapshot, None),
                (
                    source.as_str(),
                    "failed",
                    original_snapshot,
                    Some("compile_failed"),
                ),
                ("second-source", "shared-request", other_snapshot, None),
            ] {
                diesel::insert_into(print_requests::table)
                    .values((
                        print_requests::source_id.eq(source_id),
                        print_requests::request_id.eq(request_id),
                        print_requests::account_id.eq("account-1"),
                        print_requests::account.eq("team-1"),
                        print_requests::team_name.eq(team_name),
                        print_requests::seat_label.eq(seat_label),
                        print_requests::original_filename.eq(filename),
                        print_requests::source_sha256.eq("source-hash-canary"),
                        print_requests::generation_failure.eq(failure),
                    ))
                    .execute(transaction.connection())?;
            }
            for (sequence, source_id, hash, bytes, pages, ahead_tasks, ahead_pages) in [
                (5, source.as_str(), "pdf-hash-canary", 1234, 3, 1, 2),
                (2, "second-source", "other-pdf-hash", 500, 2, 0, 0),
            ] {
                diesel::insert_into(print_documents::table)
                    .values((
                        print_documents::ready_sequence.eq(sequence),
                        print_documents::source_id.eq(source_id),
                        print_documents::request_id.eq("shared-request"),
                        print_documents::sha256.eq(hash),
                        print_documents::bytes.eq(bytes),
                        print_documents::pages.eq(pages),
                        print_documents::ahead_tasks.eq(ahead_tasks),
                        print_documents::ahead_pages.eq(ahead_pages),
                    ))
                    .execute(transaction.connection())?;
            }
            diesel::delete(accounts::table).execute(transaction.connection())?;
            Ok::<_, PrintError>(())
        })
        .await
        .map_err(|source| QueueTestError::from(crate::test_error::TestError::from(source)))?;
    // Queue polling reads committed metadata, not the PDF directory or live roster.
    std::fs::remove_dir_all(f.database.documents_path())?;
    let response = read_queue(&f).await?;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&response.body)?,
        json!([
            {"source_id":"second-source", "source_name":"Second judge", "request_id":"shared-request", "account":"team-1", "team_name":"Other team", "seat_label":null, "original_filename":"other.cpp", "ready_sequence":2, "pages":2, "bytes":500},
            {"source_id":f.source_id, "source_name":"domjudge-primary", "request_id":"shared-request", "account":"team-1", "team_name":"原队伍", "seat_label":"A-01", "original_filename":"main.cpp", "ready_sequence":5, "pages":3, "bytes":1234}
        ])
    );
    let text = String::from_utf8_lossy(&response.body);
    for secret in [
        &f.token,
        "token_hash",
        "source-hash-canary",
        "pdf-hash-canary",
    ] {
        assert!(!text.contains(secret));
    }
    Ok(())
}
