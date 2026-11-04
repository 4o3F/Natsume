//! Thin administrator adapter for acceptance and optimistic decisions.
use crate::component::photo::PhotoComponent;
use crate::{
    component::photo::{PendingPhotoQuery, PhotoDecision},
    http::{
        Routes,
        error::ApiError,
        middleware::{self, audit::canonical_uuid},
    },
};
use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use std::sync::Arc;

pub(crate) fn routes() -> Routes<Arc<PhotoComponent>> {
    vec![
        (
            "/photo/submissions",
            middleware::require_admin(get(pending)),
        ),
        (
            "/photo/imports/{import_id}/preview",
            middleware::require_admin(get(preview)),
        ),
        (
            "/photo/imports/{import_id}/commit",
            middleware::require_admin(post(commit)),
        ),
        (
            "/photo/imports/{import_id}/decisions",
            middleware::require_admin(post(decide)),
        ),
    ]
}

async fn pending(
    State(accept): State<Arc<PhotoComponent>>,
    query: Result<Query<PendingPhotoQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return ApiError::invalid_request("photo_pending_query").into_response();
    };
    match accept.pending(query).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => ApiError::from_photo(error).into_response(),
    }
}

fn id(path: Result<Path<String>, PathRejection>) -> Option<uuid::Uuid> {
    let Path(raw) = path.ok()?;
    canonical_uuid(&raw)
}
async fn preview(
    State(accept): State<Arc<PhotoComponent>>,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    let Some(id) = id(path) else {
        return ApiError::invalid_request("photo_import_id").into_response();
    };
    match accept.preview(id).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => ApiError::from_photo(error).into_response(),
    }
}
async fn commit(
    State(accept): State<Arc<PhotoComponent>>,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    let Some(id) = id(path) else {
        return ApiError::invalid_request("photo_import_id").into_response();
    };
    match accept.commit(id).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => ApiError::from_photo(error).into_response(),
    }
}
async fn decide(
    State(accept): State<Arc<PhotoComponent>>,
    path: Result<Path<String>, PathRejection>,
    body: Result<Json<PhotoDecision>, JsonRejection>,
) -> Response {
    let Some(id) = id(path) else {
        return ApiError::invalid_request("photo_import_id").into_response();
    };
    let Ok(Json(decision)) = body else {
        return ApiError::invalid_request("photo_decision").into_response();
    };
    match accept.decide(id, decision).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => ApiError::from_photo(error).into_response(),
    }
}
