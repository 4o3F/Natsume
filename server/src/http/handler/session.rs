use axum::{
    Extension, Json, Router,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::component::operator::OperatorIdentity;

use super::super::{AppState, cookie, error::ApiError, middleware};

pub(in crate::http) fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/session", post(create_session))
        .route("/session", delete(delete_session))
}

pub(in crate::http) fn protected_routes(state: AppState) -> Router<AppState> {
    Router::new().route(
        "/session",
        middleware::require_operator(state, get(read_session)),
    )
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionRequest {
    login_name: String,
    #[schema(write_only)]
    password: String,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionResponse {
    operator_id: Uuid,
    role: &'static str,
}

#[utoipa::path(
    post,
    path = "/api/v2/session",
    operation_id = "createSession",
    request_body = SessionRequest,
    responses(
        (status = 200, description = "Session established", body = SessionResponse),
        (status = 400, description = "Invalid closed request"),
        (status = 401, description = "Authentication failed"),
        (status = 413, description = "Request body exceeds the API ingress limit"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn create_session(
    State(state): State<AppState>,
    request: Result<Json<SessionRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return ApiError::invalid_request("session_request_body_rejected").into_response();
    };
    let signed_in = match state
        .operator()
        .sign_in(&request.login_name, request.password)
        .await
    {
        Ok(signed_in) => signed_in,
        Err(error) => return ApiError::from_operator(error).into_response(),
    };
    let wire_credential = signed_in.wire_credential();
    let Ok(session_cookie) = cookie::issue_session_credential(wire_credential.expose()) else {
        return ApiError::internal_error("session_cookie_issuance_failed").into_response();
    };
    identity_response(signed_in.identity(), Some(session_cookie))
}

#[utoipa::path(
    get,
    path = "/api/v2/session",
    operation_id = "getSession",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current session", body = SessionResponse),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn read_session(Extension(identity): Extension<OperatorIdentity>) -> Response {
    identity_response(identity, None)
}

#[utoipa::path(
    delete,
    path = "/api/v2/session",
    operation_id = "deleteSession",
    security(("sessionCookie" = [])),
    responses(
        (status = 204, description = "Session terminated or credential-state no-op"),
        (status = 500, description = "Session termination infrastructure failure")
    )
)]
pub(crate) async fn delete_session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let response = match cookie::session_credential(&headers) {
        Ok(wire_credential) => match state.operator().terminate_session(wire_credential).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(error) => ApiError::from_operator(error).into_response(),
        },
        Err(()) => StatusCode::NO_CONTENT.into_response(),
    };
    cookie::with_clearing_session_cookie(response).unwrap_or_else(|()| {
        ApiError::internal_error("session_clearing_cookie_failed").into_response()
    })
}

fn identity_response(identity: OperatorIdentity, session_cookie: Option<HeaderValue>) -> Response {
    let body = SessionResponse {
        operator_id: identity.operator_id(),
        role: identity.role_name(),
    };
    let encoded = serde_json::to_string(&body).unwrap_or_else(|_| {
        tracing::error!("session response serialization invariant failed");
        panic!("session response serialization invariant failed");
    });
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        encoded,
    )
        .into_response();
    if let Some(session_cookie) = session_cookie {
        response
            .headers_mut()
            .insert(header::SET_COOKIE, session_cookie);
    }
    response
}
