use super::super::{
    LOGIN_NAME, PASSWORD, PasswordVerificationTestGuard, header_text, login_request, seed_operator,
};
use super::*;
use std::{fs, io, os::unix::fs::PermissionsExt as _};

async fn admin() -> Result<(Fixture, String), TestError> {
    let fixture = Fixture::new().await?;
    seed_operator(&fixture.database.database, LOGIN_NAME, PASSWORD).await?;
    let login = drive(&fixture.app, login_request(LOGIN_NAME, PASSWORD)?).await?;
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or(TestError::missing("cookie"))?
        .to_owned();
    Ok((fixture, cookie))
}

fn upload(cookie: &str, body: Body) -> Result<Request<Body>, axum::http::Error> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/v2/photo/imports")
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/zip")
        .body(body)
}

#[tokio::test]
async fn streamed_upload_is_durable_queryable_bounded_and_not_verified() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (f, cookie) = admin().await?;
    let chunks = [
        Ok::<_, io::Error>(axum::body::Bytes::from_static(b"PK")),
        Ok(axum::body::Bytes::from_static(b"upload fixture")),
    ];
    let accepted = drive(
        &f.app,
        upload(
            &cookie,
            Body::from_stream(futures_util::stream::iter(chunks)),
        )?,
    )
    .await?;
    assert_eq!(accepted.status, StatusCode::ACCEPTED);
    let job: Value = serde_json::from_slice(&accepted.body)?;
    assert_eq!(job["state"], "uploaded");
    assert_eq!(job["uploaded_bytes"], 16);
    assert_eq!(job["verified_images"], 0);
    let id = job["import_id"]
        .as_str()
        .ok_or(TestError::missing("import ID"))?;
    let root = f
        .database
        .documents_path()
        .with_extension("photo-results")
        .join("uploads");
    let file = root.join(format!("{id}.zip"));
    assert_eq!(fs::read(&file)?, b"PKupload fixture");
    assert_eq!(fs::metadata(file)?.permissions().mode() & 0o777, 0o600);
    let read = drive(
        &f.app,
        Request::builder()
            .uri(format!("/api/v2/photo/imports/{id}"))
            .header(header::COOKIE, &cookie)
            .body(Body::empty())?,
    )
    .await?;
    assert_eq!(read.status, StatusCode::OK);
    assert_eq!(serde_json::from_slice::<Value>(&read.body)?, job);
    let busy = drive(&f.app, upload(&cookie, Body::from("another"))?).await?;
    assert_eq!(busy.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(busy.headers[header::RETRY_AFTER], "1");
    assert_eq!(fs::read_dir(root)?.count(), 1);
    Ok(())
}

#[tokio::test]
async fn upload_rejects_unauthorized_invalid_and_empty_input_without_retaining_files() -> TestResult
{
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (f, cookie) = admin().await?;
    assert_eq!(
        drive(&f.app, upload("", Body::from("secret"))?)
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    let mut wrong_type = upload(&cookie, Body::from("secret"))?;
    wrong_type.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    assert_eq!(
        drive(&f.app, wrong_type).await?.status,
        StatusCode::BAD_REQUEST
    );
    let mut oversized = upload(&cookie, Body::empty())?;
    oversized.headers_mut().insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from_static("8589934593"),
    );
    assert_eq!(
        drive(&f.app, oversized).await?.status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        drive(&f.app, upload(&cookie, Body::empty())?).await?.status,
        StatusCode::BAD_REQUEST
    );
    let root = f
        .database
        .documents_path()
        .with_extension("photo-results")
        .join("uploads");
    assert_eq!(fs::read_dir(root)?.count(), 0);
    for id in ["invalid".to_owned(), Uuid::now_v7().to_string()] {
        let response = drive(
            &f.app,
            Request::builder()
                .uri(format!("/api/v2/photo/imports/{id}"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
        assert_eq!(
            response.status,
            if id == "invalid" {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::NOT_FOUND
            }
        );
        assert_eq!(response.headers[header::CONTENT_TYPE], "application/json");
    }
    Ok(())
}

#[tokio::test]
async fn broken_input_cleans_partial_file_and_releases_capacity() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (f, cookie) = admin().await?;
    let chunks = [
        Ok(axum::body::Bytes::from_static(b"partial")),
        Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "private input canary",
        )),
    ];
    let failure = drive(
        &f.app,
        upload(
            &cookie,
            Body::from_stream(futures_util::stream::iter(chunks)),
        )?,
    )
    .await?;
    assert_eq!(failure.status, StatusCode::BAD_REQUEST);
    assert!(!String::from_utf8_lossy(&failure.body).contains("canary"));
    let root = f
        .database
        .documents_path()
        .with_extension("photo-results")
        .join("uploads");
    assert_eq!(fs::read_dir(root)?.count(), 0);
    assert_eq!(
        drive(&f.app, upload(&cookie, Body::from("new upload"))?)
            .await?
            .status,
        StatusCode::ACCEPTED
    );
    Ok(())
}

#[tokio::test]
async fn disconnected_caller_and_shutdown_keep_upload_ownership_until_cleanup() -> TestResult {
    use crate::diesel_schema::photo_import_jobs as jobs;
    use diesel::OptionalExtension as _;
    use std::{sync::Arc, time::Duration};
    use tokio::sync::Notify;
    use tower::ServiceExt as _;
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (f, cookie) = admin().await?;
    let release = Arc::new(Notify::new());
    let gate = release.clone();
    let stream = futures_util::stream::unfold(false, move |sent| {
        let gate = gate.clone();
        async move {
            if sent {
                gate.notified().await;
                None
            } else {
                Some((
                    Ok::<_, io::Error>(axum::body::Bytes::from(vec![0; 1024 * 1024])),
                    true,
                ))
            }
        }
    });
    let request = upload(&cookie, Body::from_stream(stream))?;
    let app = f.app.clone();
    let caller = tokio::spawn(async move { app.oneshot(request).await });
    let id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let current = f
                .database
                .database
                .read(|tx| {
                    jobs::table
                        .filter(jobs::uploaded_bytes.ge(1024 * 1024))
                        .select(jobs::import_id)
                        .first::<String>(tx.connection())
                        .optional()
                        .map_err(PersistenceError::from)
                })
                .await
                .map_err(crate::db::TransactionError::into_error)?;
            if let Some(id) = current {
                return Ok::<_, TestError>(id);
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    caller.abort();
    assert_eq!(
        drive(&f.app, upload(&cookie, Body::from("another"))?)
            .await?
            .status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    f.lifecycle.begin_shutdown();
    tokio::time::timeout(Duration::from_secs(5), f.lifecycle.wait()).await?;
    let state = f
        .database
        .database
        .read(move |tx| {
            jobs::table
                .find(id)
                .select(jobs::state)
                .first::<String>(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert_eq!(state, "failed");
    assert_eq!(
        fs::read_dir(
            f.database
                .documents_path()
                .with_extension("photo-results")
                .join("uploads")
        )?
        .count(),
        0
    );
    release.notify_one();
    Ok(())
}
