//! Authorized transport with tracked, bounded download producers.
use crate::component::photo::limits::{IO_BUFFER_BYTES, OPERATION_TIMEOUT};
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
    http::header,
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use futures_util::StreamExt as _;
use tokio_util::io::ReaderStream;

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
        Err(error) => ApiError::from_photo_delivery(error).into_response(),
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
        Err(error) => ApiError::from_photo_delivery(error).into_response(),
    }
}
async fn export(State(deliver): State<DeliverPhotoResults>) -> Response {
    match deliver.export().await {
        Ok((ready, admission)) => response(
            ready,
            admission,
            Some("attachment; filename=\"team-photos.zip\""),
        ),
        Err(error) => ApiError::from_photo_delivery(error).into_response(),
    }
}

async fn export_pyrite(State(deliver): State<DeliverPhotoResults>) -> Response {
    match deliver.export_pyrite().await {
        Ok((ready, admission)) => response(
            ready,
            admission,
            Some("attachment; filename=\"pyrite-assets.zip\""),
        ),
        Err(error) => ApiError::from_photo_delivery(error).into_response(),
    }
}

fn response(
    ready: PreparedDownload,
    admission: Admission,
    attachment: Option<&'static str>,
) -> Response {
    let length = ready.length;
    let content_type = ready.content_type;
    let reader = ReaderStream::with_capacity(
        DownloadFile {
            file: Some(tokio::fs::File::from_std(ready.file)),
            permit: Some(ready.permit),
            admission: admission.clone(),
        },
        IO_BUFFER_BYTES,
    );
    let deadline = tokio::time::Instant::now() + OPERATION_TIMEOUT;
    let stream = futures_util::stream::unfold(Some((reader, admission)), move |state| async move {
        let (mut reader, admission) = state?;
        let next = tokio::select! {
            () = admission.stopping() => Some(Err(std::io::Error::new(std::io::ErrorKind::Interrupted,"Photo download stopped"))),
            next = tokio::time::timeout_at(deadline,reader.next()) => match next {
                Ok(next) => next,
                Err(source) => Some(Err(std::io::Error::new(std::io::ErrorKind::TimedOut,source))),
            },
        };
        match next {
            Some(Ok(bytes)) => Some((Ok(bytes), Some((reader, admission)))),
            Some(Err(error)) => {
                tracing::warn!(%error,"Photo download interrupted");
                Some((Err(error), None))
            }
            None => None,
        }
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

// Keep the slot on the actual reader, rather than only on its response body.
struct DownloadFile {
    file: Option<tokio::fs::File>,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    admission: Admission,
}
impl tokio::io::AsyncRead for DownloadFile {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let Some(file) = this.file.as_mut() else {
            return std::task::Poll::Ready(Ok(()));
        };
        let before = buffer.filled().len();
        let result = std::pin::Pin::new(file).poll_read(cx, buffer);
        if result.is_ready()
            && (matches!(&result, std::task::Poll::Ready(Err(_)))
                || buffer.filled().len() == before)
        {
            // EOF/error means this file has no pending read; release immediately.
            drop(this.permit.take());
        }
        result
    }
}
impl Drop for DownloadFile {
    fn drop(&mut self) {
        if let (Some(file), Some(permit)) = (self.file.take(), self.permit.take()) {
            let admission = self.admission.clone();
            // An aborted body may leave a blocking Tokio read in flight. Wait
            // for it in a tracked task before releasing either ownership token.
            drop(self.admission.spawn(async move {
                drop(file.into_std().await);
                drop(permit);
                drop(admission);
            }));
        }
    }
}
