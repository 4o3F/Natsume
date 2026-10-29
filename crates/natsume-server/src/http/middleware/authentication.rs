use crate::http::middleware::audit::RequestAudit;
use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::super::{AppState, cookie, error::ApiError};

pub(in crate::http) async fn authenticate_operator(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Ok(wire_credential) = cookie::session_credential(request.headers()) else {
        return ApiError::authentication_failed("missing_session_cookie").into_response();
    };
    let identity = match state.operator().authenticate_session(wire_credential).await {
        Ok(identity) => identity,
        Err(error) => return ApiError::from_operator(error).into_response(),
    };
    if let Some(audit) = request.extensions().get::<RequestAudit>() {
        audit.record_actor(identity);
    }
    let actor_id = identity.operator_id().to_string();
    tracing::Span::current()
        .record("actor_kind", "operator")
        .record("actor_id", actor_id.as_str());
    request.extensions_mut().insert(identity);
    next.run(request).await
}

pub(in crate::http) async fn authenticate_source(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let mut auth = headers.get_all(header::AUTHORIZATION).iter();
    let token = auth
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) = token.filter(|_| auth.next().is_none()) else {
        return ApiError::from_print(crate::component::print::PrintError::Authentication)
            .into_response();
    };
    match state.print().authenticate(token).await {
        Ok(identity) => {
            request.extensions_mut().insert(identity);
            next.run(request).await
        }
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

pub(in crate::http) async fn authenticate_client(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let mut values = request.headers().get_all(header::AUTHORIZATION).iter();
    let token = values
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) = token.filter(|_| values.next().is_none()) else {
        return ApiError::from_print(crate::component::print::PrintError::Authentication)
            .into_response();
    };
    match state.print().authenticate_client(token).await {
        Ok(client) => {
            request.extensions_mut().insert(client);
            next.run(request).await
        }
        Err(error) => ApiError::from_print(error).into_response(),
    }
}
