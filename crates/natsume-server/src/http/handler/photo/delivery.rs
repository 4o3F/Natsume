//! Authorized transport with tracked, bounded download producers.
use crate::{
    component::photo::PreparedDownload,
    http::{Routes, error::ApiError, middleware},
    lifecycle::Admission,
    photo_delivery::DeliverPhotoResults,
};
use axum::{
    Json,
    body::Body,
    extract::{Path, State, rejection::PathRejection},
    http::{StatusCode, header},
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use std::io::Read as _;

pub(crate) fn routes() -> Routes<DeliverPhotoResults> {
    vec![
        ("/photo/photos", middleware::require_admin(get(list))),
        (
            "/photo/photos/{account_id}/image",
            middleware::require_admin(get(image)),
        ),
        ("/photo/exports", middleware::require_admin(post(export))),
        (
            "/photo/exports/pyrite",
            middleware::require_admin(post(export_pyrite)),
        ),
    ]
}
async fn list(State(deliver): State<DeliverPhotoResults>) -> Response {
    match deliver.list().await {
        Ok(value) => Json(value).into_response(),
        Err(error) => failure(error),
    }
}
async fn image(
    State(deliver): State<DeliverPhotoResults>,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    let Ok(Path(id)) = path else {
        return ApiError::invalid_request("photo_account_id").into_response();
    };
    match deliver.image(id).await {
        Ok((ready, admission)) => response(ready, admission, None),
        Err(error) => failure(error),
    }
}
async fn export(State(deliver): State<DeliverPhotoResults>) -> Response {
    match deliver.export().await {
        Ok((ready, admission)) => response(
            ready,
            admission,
            Some("attachment; filename=\"team-photos.zip\""),
        ),
        Err(error) => failure(error),
    }
}

async fn export_pyrite(State(deliver): State<DeliverPhotoResults>) -> Response {
    match deliver.export_pyrite().await {
        Ok((ready, admission)) => response(
            ready,
            admission,
            Some("attachment; filename=\"pyrite-assets.zip\""),
        ),
        Err(error) => failure(error),
    }
}

fn failure(error: crate::photo_delivery::Error) -> Response {
    let mut response = ApiError::from_photo_delivery(error).into_response();
    if response.status() == StatusCode::SERVICE_UNAVAILABLE {
        response.headers_mut().insert(
            header::RETRY_AFTER,
            axum::http::HeaderValue::from_static("1"),
        );
    }
    response
}

fn response(
    ready: PreparedDownload,
    admission: Admission,
    attachment: Option<&'static str>,
) -> Response {
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(2);
    let length = ready.length;
    let content_type = ready.content_type;
    let tracking = admission.clone();
    // The tracked async task joins the real blocking reader; its closure owns
    // the file, admission and permit until physical IO has finished.
    drop(tracking.spawn(async move {
        let result = tokio::task::spawn_blocking(move || pump(ready, &admission, &sender)).await;
        if let Err(source) = result {
            tracing::error!(error = %crate::task::TaskFailure::from(source), "Photo download worker failed");
        }
    }));
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });
    let mut response = (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(stream),
    )
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        axum::http::HeaderValue::from(length),
    );
    if let Some(attachment) = attachment {
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            axum::http::HeaderValue::from_static(attachment),
        );
    }
    response
}

fn pump(
    ready: PreparedDownload,
    admission: &Admission,
    sender: &tokio::sync::mpsc::Sender<Result<Vec<u8>, std::io::Error>>,
) {
    let _permit = ready.permit;
    let mut file = ready.file;
    let runtime = tokio::runtime::Handle::current();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_mins(15);
    loop {
        if admission.is_stopping() || tokio::time::Instant::now() >= deadline {
            break;
        }
        let mut buffer = vec![0_u8; 64 * 1024];
        let count = match file.read(&mut buffer) {
            Ok(count) => count,
            Err(error) => {
                tracing::error!(%error, "Photo download failed");
                break;
            }
        };
        if count == 0 {
            return;
        }
        buffer.truncate(count);
        let sent = runtime.block_on(async {
            tokio::select! {
                () = admission.stopping() => false,
                result = tokio::time::timeout_at(deadline, sender.send(Ok(buffer))) => matches!(result, Ok(Ok(()))),
            }
        });
        if !sent {
            break;
        }
    }
    let _sent = sender.try_send(Err(std::io::Error::other("Photo download interrupted")));
}
