use crate::http::Routes;
use crate::http::middleware::audit::RequestAudit;

use axum::{
    Extension, Json,
    extract::{FromRequest, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::component::operator::OperatorIdentity;

use super::super::{AppState, cookie, error::ApiError};

pub(in crate::http) fn public_routes() -> Routes {
    vec![("/session", post(create_session).delete(delete_session))]
}

pub(in crate::http) fn operator_routes() -> Routes {
    vec![("/session", get(read_session))]
}

#[derive(Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionRequest {
    /// Nonempty login name, at most 128 UTF-8 bytes; never normalized or truncated.
    login_name: String,
    /// At most 1024 UTF-8 bytes; never normalized or truncated.
    password: String,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionResponse {
    operator_id: Uuid,
    username: String,
    role: &'static str,
}

pub(in crate::http) async fn create_session(
    Extension(audit): Extension<RequestAudit>,
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let Ok(Json(request)) = Json::<SessionRequest>::from_request(request, &state).await else {
        return ApiError::invalid_request("session_request_body_rejected").into_response();
    };
    let signed_in = match state
        .operator()
        .sign_in(&request.login_name, request.password, audit.context())
        .await
    {
        Ok(signed_in) => signed_in,
        Err(error) => return ApiError::from_operator(error).into_response(),
    };
    audit.record_actor(signed_in.identity());
    let wire_credential = signed_in.wire_credential();
    let session_cookie = match cookie::issue_session_credential(wire_credential.expose()) {
        Ok(value) => value,
        Err(error) => {
            return ApiError::internal_error_from(&error, "session_cookie_issuance_failed")
                .into_response();
        }
    };
    identity_response(&state, signed_in.identity(), Some(session_cookie)).await
}

pub(crate) async fn read_session(
    State(state): State<AppState>,
    Extension(identity): Extension<OperatorIdentity>,
) -> Response {
    identity_response(&state, identity, None).await
}

pub(in crate::http) async fn delete_session(
    Extension(audit): Extension<RequestAudit>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let response = match cookie::session_credential(&headers) {
        Ok(wire_credential) => match state
            .operator()
            .terminate_session(wire_credential, audit.context())
            .await
        {
            Ok(Some(identity)) => {
                audit.record_actor(identity);
                StatusCode::NO_CONTENT.into_response()
            }
            Ok(None) => StatusCode::NO_CONTENT.into_response(),
            Err(error) => ApiError::from_operator(error).into_response(),
        },
        Err(()) => StatusCode::NO_CONTENT.into_response(),
    };
    cookie::with_clearing_session_cookie(response).unwrap_or_else(|error| {
        ApiError::internal_error_from(&error, "session_clearing_cookie_failed").into_response()
    })
}

async fn identity_response(
    state: &AppState,
    identity: OperatorIdentity,
    session_cookie: Option<HeaderValue>,
) -> Response {
    let username = match state.operator().session_username(identity).await {
        Ok(username) => username,
        Err(error) => return ApiError::from_operator(error).into_response(),
    };
    let body = SessionResponse {
        operator_id: identity.operator_id(),
        username,
        role: identity.role_name(),
    };
    let mut response = Json(body).into_response();
    if let Some(session_cookie) = session_cookie {
        response
            .headers_mut()
            .insert(header::SET_COOKIE, session_cookie);
    }
    response
}
