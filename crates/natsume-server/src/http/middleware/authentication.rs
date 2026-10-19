use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::super::{AppState, cookie, error::ApiError};

pub(super) async fn authenticate_operator(
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
    let actor_id = identity.operator_id().to_string();
    tracing::Span::current()
        .record("actor_kind", "operator")
        .record("actor_id", actor_id.as_str());
    request.extensions_mut().insert(identity);
    next.run(request).await
}
