use crate::http::Routes;
use crate::http::middleware::audit::{RequestAudit, canonical_uuid};

mod dto;
mod execution;
mod printers;

pub(crate) use dto::{
    PrintGenerationFailure, PrintRegistration, PrintRequest, PrintSource, PrintSourceIssued,
};

use axum::{
    Extension, Json,
    body::Bytes,
    extract::{
        DefaultBodyLimit, FromRequestParts, Path, State,
        rejection::{BytesRejection, JsonRejection},
    },
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse as _, Response},
    routing::{delete, get, post, put},
};
use serde::Deserialize;

use super::super::{AppState, error::ApiError, middleware};
use crate::component::print::{DOCUMENT_LIMIT, PrintError, SourceIdentity, canonical_id};

pub(in crate::http) fn source_routes() -> Routes {
    vec![
        ("/print/requests", post(register)),
        ("/print/requests/{request_id}", get(get_request)),
        (
            "/print/requests/{request_id}/generation-failure",
            post(generation_failure),
        ),
        (
            "/print/requests/{request_id}/document",
            put(upload).layer(DefaultBodyLimit::max(DOCUMENT_LIMIT)),
        ),
    ]
}

pub(in crate::http) fn operator_routes() -> Routes {
    let mut routes = vec![
        ("/print/queue", get(list_queue)),
        (
            "/print/sources",
            middleware::require_admin(post(create_source).get(list_sources)),
        ),
        (
            "/print/sources/{source_id}",
            middleware::require_admin(delete(revoke_source)),
        ),
    ];
    routes.extend(printers::operator_routes());
    routes.extend(execution::operator_routes());
    routes
}

pub(in crate::http) fn public_routes(state: AppState) -> Routes {
    printers::public_routes(state)
}

pub(in crate::http) fn client_routes() -> Routes {
    execution::client_routes()
}

#[derive(Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintSourceCreate {
    name: String,
}

/// Canonical path IDs reject Axum decoding failures using the public error envelope.
pub(crate) struct PrintPathId(String);
impl<S: Send + Sync> FromRequestParts<S> for PrintPathId {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(id) = Path::<String>::from_request_parts(parts, state)
            .await
            // Malformed request input is a domain rejection; parser detail is irrelevant.
            .map_err(|_| ApiError::invalid_request("print_path").into_response())?;
        canonical_id(&id).map_err(|error| ApiError::from_print(error).into_response())?;
        Ok(Self(id))
    }
}

pub(crate) async fn list_queue(State(state): State<AppState>) -> Response {
    match state.print().list_queue().await {
        Ok(queue) => Json(queue).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

pub(in crate::http) async fn create_source(
    Extension(audit): Extension<RequestAudit>,
    State(state): State<AppState>,
    body: Result<Json<PrintSourceCreate>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return ApiError::invalid_request("print_json").into_response();
    };
    match state.print().create_source(body.name).await {
        Ok(source) => {
            if let Some(id) = canonical_uuid(&source.source_id) {
                audit.record_target(crate::journal::event::types::AuditTarget::PrintSource(id));
            }
            Json(PrintSourceIssued::from(source)).into_response()
        }
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

pub(crate) async fn list_sources(State(state): State<AppState>) -> Response {
    match state.print().list_sources().await {
        Ok(sources) => Json(
            sources
                .into_iter()
                .map(PrintSource::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

pub(crate) async fn revoke_source(
    State(state): State<AppState>,
    PrintPathId(id): PrintPathId,
) -> Response {
    match state.print().revoke_source(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::from_print(e).into_response(),
    }
}

pub(in crate::http) async fn register(
    State(state): State<AppState>,
    Extension(source): Extension<SourceIdentity>,
    body: Result<Json<PrintRegistration>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return ApiError::invalid_request("print_json").into_response();
    };
    match state.print().register(source, body.into()).await {
        Ok(request) => Json(PrintRequest::from(request)).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

pub(crate) async fn get_request(
    State(state): State<AppState>,
    Extension(source): Extension<SourceIdentity>,
    PrintPathId(id): PrintPathId,
) -> Response {
    match state.print().get(source, id).await {
        Ok(request) => Json(PrintRequest::from(request)).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

pub(crate) async fn generation_failure(
    State(state): State<AppState>,
    Extension(source): Extension<SourceIdentity>,
    PrintPathId(id): PrintPathId,
    body: Result<Json<PrintGenerationFailure>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return ApiError::invalid_request("print_json").into_response();
    };
    match state
        .print()
        .generation_failure(source, id, body.into())
        .await
    {
        Ok(request) => Json(PrintRequest::from(request)).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

pub(crate) async fn upload(
    State(state): State<AppState>,
    Extension(source): Extension<SourceIdentity>,
    PrintPathId(id): PrintPathId,
    headers: axum::http::HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        != Some("application/pdf")
    {
        return ApiError::invalid_request("print_pdf_content_type").into_response();
    }
    let bytes = match body {
        Ok(bytes) => bytes,
        Err(e) if e.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return ApiError::from_print(PrintError::TooLarge).into_response();
        }
        Err(_) => return ApiError::invalid_request("print_pdf_body").into_response(),
    };
    match state.print().upload(source, id, bytes.to_vec()).await {
        Ok(request) => Json(PrintRequest::from(request)).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}
