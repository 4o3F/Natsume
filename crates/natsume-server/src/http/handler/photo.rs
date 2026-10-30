//! Phase-specific Photo authentication; no Operator credential reaches a client.
use axum::{
    Extension, Json,
    body::{Body, Bytes},
    extract::{
        ConnectInfo, DefaultBodyLimit, FromRequest as _, Path, Request, State,
        rejection::JsonRejection,
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use serde::Deserialize;

use crate::http::middleware::audit::{RequestAudit, canonical_uuid};
use crate::{
    component::photo::{CredentialUse, PendingEnrollment, SourceState},
    http::{AppState, Routes, error::ApiError, middleware},
    journal::event::types::AuditTarget,
    tls::PeerAddress,
};

#[derive(Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhotoSourceCreate {
    name: String,
}
#[derive(Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhotoSourceStateUpdate {
    state: SourceState,
}
#[derive(Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhotoPairingCodeRequest {}
pub(crate) use natsume_photo_protocol::{
    PhotoConfirmation, PhotoInitializationComplete, PhotoPairRequest,
};

pub(in crate::http) fn operator_routes() -> Routes {
    vec![
        (
            "/photo/sources",
            middleware::require_admin(post(create_source).get(list_sources)),
        ),
        (
            "/photo/sources/{source_id}/pairing-code",
            middleware::require_admin(post(issue_code)),
        ),
        (
            "/photo/sources/{source_id}/reset-enrollment",
            middleware::require_admin(post(reset_enrollment)),
        ),
        (
            "/photo/sources/{source_id}/state",
            middleware::require_admin(post(update_state)),
        ),
    ]
}

/// Publicly mounted, like Print confirmation. Every route except pairing
/// authenticates its Photo-only credential before accessing protected state.
pub(in crate::http) fn enrollment_routes() -> Routes {
    vec![
        ("/photo/enrollment/pair", post(pair)),
        ("/photo/enrollment/confirm", post(confirm)),
        ("/photo/client/initialization", post(initialization)),
        ("/photo/client/resources/{sha256}", get(resource)),
        ("/photo/client/initialization/complete", post(complete)),
    ]
    .into_iter()
    .map(|(path, routes)| (path, routes.layer(DefaultBodyLimit::max(8192))))
    .collect()
}

fn bearer(headers: &HeaderMap) -> Result<&str, ApiError> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let token = values
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    token
        .filter(|_| values.next().is_none())
        .ok_or_else(|| ApiError::authentication_failed("photo_missing_credential"))
}

async fn create_source(
    Extension(audit): Extension<RequestAudit>,
    State(state): State<AppState>,
    body: Result<Json<PhotoSourceCreate>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return ApiError::invalid_request("photo_source_json").into_response();
    };
    match state.photo().create_source(body.name).await {
        Ok(source) => {
            if let Some(id) = canonical_uuid(source.source_id.as_str()) {
                audit.record_target(AuditTarget::PhotoSource(id));
            }
            (StatusCode::CREATED, Json(source)).into_response()
        }
        Err(error) => ApiError::from_photo_enrollment(error).into_response(),
    }
}
async fn list_sources(State(state): State<AppState>) -> Response {
    match state.photo().list_sources().await {
        Ok(sources) => Json(sources).into_response(),
        Err(error) => ApiError::from_photo_enrollment(error).into_response(),
    }
}
async fn issue_code(
    State(state): State<AppState>,
    Path(id): Path<natsume_photo_protocol::SourceId>,
    body: Result<Json<PhotoPairingCodeRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(_body)) = body else {
        return ApiError::invalid_request("photo_pairing_code_json").into_response();
    };
    match state
        .photo()
        .issue_code(id, PendingEnrollment::Preserve)
        .await
    {
        Ok(code) => Json(code).into_response(),
        Err(error) => ApiError::from_photo_enrollment(error).into_response(),
    }
}
async fn reset_enrollment(
    State(state): State<AppState>,
    Path(id): Path<natsume_photo_protocol::SourceId>,
) -> Response {
    match state
        .photo()
        .issue_code(id, PendingEnrollment::Replace)
        .await
    {
        Ok(code) => Json(code).into_response(),
        Err(error) => ApiError::from_photo_enrollment(error).into_response(),
    }
}
async fn update_state(
    State(state): State<AppState>,
    Path(id): Path<natsume_photo_protocol::SourceId>,
    body: Result<Json<PhotoSourceStateUpdate>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return ApiError::invalid_request("photo_source_state_json").into_response();
    };
    match state.photo().set_state(id, body.state).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_photo_enrollment(error).into_response(),
    }
}
async fn pair(State(state): State<AppState>, request: Request) -> Response {
    let Some(ConnectInfo(PeerAddress(peer))) =
        request.extensions().get::<ConnectInfo<PeerAddress>>()
    else {
        return ApiError::unavailable("photo_pairing_peer_unavailable").into_response();
    };
    if let Err(error) = state.photo().admit_pairing(*peer) {
        return ApiError::from_photo_enrollment(error).into_response();
    }
    let Ok(Json(body)) = Json::<PhotoPairRequest>::from_request(request, &state).await else {
        return ApiError::invalid_request("photo_pairing_json").into_response();
    };
    match state.photo().pair(body).await {
        Ok(issued) => (StatusCode::CREATED, Json(issued)).into_response(),
        Err(error) => ApiError::from_photo_enrollment(error).into_response(),
    }
}
async fn mutation_token(
    state: &AppState,
    headers: &HeaderMap,
    purpose: CredentialUse,
) -> Result<String, ApiError> {
    let token = bearer(headers)?.to_owned();
    state
        .photo()
        .check_token(&token, purpose)
        .await
        .map_err(ApiError::from_photo_enrollment)?;
    Ok(token)
}
async fn confirm(State(state): State<AppState>, request: Request) -> Response {
    let token = match mutation_token(&state, request.headers(), CredentialUse::Initialization).await
    {
        Ok(token) => token,
        Err(error) => return error.into_response(),
    };
    let Ok(Json(body)) = Json::<PhotoConfirmation>::from_request(request, &state).await else {
        return ApiError::invalid_request("photo_confirmation_json").into_response();
    };
    match state.photo().confirm(&token, body).await {
        Ok(source) => Json(source).into_response(),
        Err(error) => ApiError::from_photo_enrollment(error).into_response(),
    }
}
async fn initialization(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let token = match bearer(&headers) {
        Ok(token) => token,
        Err(error) => return error.into_response(),
    };
    match state.photo_initialization().run(token).await {
        Ok(download) => (
            [
                (header::CONTENT_TYPE, "application/json".to_owned()),
                (header::ETAG, format!("\"{}\"", download.sha256)),
            ],
            download.json,
        )
            .into_response(),
        Err(crate::photo_initialization::Error::Photo { source }) => initialization_error(source),
        Err(error) => {
            ApiError::internal_error_from(error, "photo_initialization_facts").into_response()
        }
    }
}
async fn resource(
    State(state): State<AppState>,
    Path(hash): Path<String>,
    headers: HeaderMap,
) -> Response {
    let token = match bearer(&headers) {
        Ok(token) => token,
        Err(error) => return error.into_response(),
    };
    match state.photo().resource(token, &hash).await {
        Ok(download) => {
            // Retain capacity until the response stream is consumed/dropped.
            let stream = futures_util::stream::unfold(
                (Some(download.bytes), download.permit),
                |(bytes, permit)| async move {
                    bytes.map(|bytes| (Ok::<_, std::io::Error>(Bytes::from(bytes)), (None, permit)))
                },
            );
            (
                [(header::CONTENT_TYPE, "image/png")],
                Body::from_stream(stream),
            )
                .into_response()
        }
        Err(error) => initialization_error(error),
    }
}
async fn complete(State(state): State<AppState>, request: Request) -> Response {
    let token = match mutation_token(&state, request.headers(), CredentialUse::Completion).await {
        Ok(token) => token,
        Err(error) => return error.into_response(),
    };
    let Ok(Json(body)) = Json::<PhotoInitializationComplete>::from_request(request, &state).await
    else {
        return ApiError::invalid_request("photo_completion_json").into_response();
    };
    match state.photo().complete(&token, body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => initialization_error(error),
    }
}

fn initialization_error(error: crate::component::photo::InitializationError) -> Response {
    let busy = matches!(error, crate::component::photo::InitializationError::Busy);
    let mut response = ApiError::from_photo_initialization(error).into_response();
    if busy {
        response.headers_mut().insert(
            header::RETRY_AFTER,
            axum::http::HeaderValue::from_static("1"),
        );
    }
    response
}
