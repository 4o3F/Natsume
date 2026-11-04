//! Operator ZIP transport; Photo owns upload admission and persistence.
use crate::{
    component::photo::{PhotoComponent, ResultImportError},
    http::{
        Routes,
        error::ApiError,
        middleware::{
            self,
            audit::{RequestAudit, canonical_uuid},
        },
    },
    journal::event::types::AuditTarget,
};
use axum::{
    Json,
    extract::{Path, Request, State, rejection::PathRejection},
    http::{StatusCode, header},
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use futures_util::StreamExt as _;
use std::{io, sync::Arc, time::Duration};

pub(crate) fn routes() -> Routes<Arc<PhotoComponent>> {
    vec![
        ("/photo/imports", middleware::require_admin(post(upload))),
        (
            "/photo/imports/{import_id}",
            middleware::require_admin(get(read)),
        ),
    ]
}

async fn upload(State(photo): State<Arc<PhotoComponent>>, request: Request) -> Response {
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/zip")
    {
        return ApiError::invalid_request("photo_zip_content_type").into_response();
    }
    if let Some(value) = request.headers().get(header::CONTENT_LENGTH) {
        let Some(length) = value
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
        else {
            return ApiError::invalid_request("photo_upload_length").into_response();
        };
        if length > natsume_photo_protocol::results::PACKAGE_LIMIT {
            return ApiError::from_photo(ResultImportError::Limit).into_response();
        }
    }
    let audit = request.extensions().get::<RequestAudit>().cloned();
    let Some(admission) = request
        .extensions()
        .get::<crate::lifecycle::Admission>()
        .cloned()
    else {
        return ApiError::unavailable("photo_upload_not_admitted").into_response();
    };
    let deadline = tokio::time::Instant::now() + crate::component::photo::limits::OPERATION_TIMEOUT;
    let stream = futures_util::stream::unfold(
        (request.into_body().into_data_stream(), deadline, admission),
        |(mut body, deadline, admission)| async move {
            let idle = tokio::time::Instant::now() + Duration::from_secs(30);
            let received = tokio::select! {
                () = admission.stopping() => return Some((Err(io::Error::new(io::ErrorKind::Interrupted, "Server stopping")), (body, deadline, admission))),
                result = tokio::time::timeout_at(deadline.min(idle), body.next()) => result,
            };
            let result = match received {
                Ok(Some(result)) => result.map_err(io::Error::other),
                Ok(None) => return None,
                Err(source) => Err(io::Error::new(io::ErrorKind::TimedOut, source)),
            };
            Some((result, (body, deadline, admission)))
        },
    );
    let input = tokio_util::io::StreamReader::new(Box::pin(stream));
    match photo.upload_results(input).await {
        Ok(job) => {
            if let Some(audit) = audit {
                audit.record_target(AuditTarget::PhotoImport(job.import_id));
            }
            (StatusCode::ACCEPTED, Json(job)).into_response()
        }
        Err(error) => ApiError::from_photo(error).into_response(),
    }
}

async fn read(
    State(photo): State<Arc<PhotoComponent>>,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    let Ok(Path(raw)) = path else {
        return ApiError::invalid_request("photo_import_id").into_response();
    };
    let Some(id) = canonical_uuid(&raw) else {
        return ApiError::invalid_request("photo_import_id").into_response();
    };
    match photo.read_import(id).await {
        Ok(job) => Json(job).into_response(),
        Err(error) => ApiError::from_photo(error).into_response(),
    }
}
