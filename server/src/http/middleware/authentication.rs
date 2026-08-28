use axum::{
    Extension,
    extract::{Request, State},
    http::Method,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::audit::CorrelationId;

use super::super::{AppState, cookie, error::ApiError};

pub(super) async fn authenticate_operator(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    mut request: Request,
    next: Next,
) -> Response {
    if request.method() == Method::HEAD {
        return next.run(request).await;
    }
    let Ok(wire_credential) = cookie::session_credential(request.headers()) else {
        return ApiError::authentication_failed("missing_session_cookie", correlation_id)
            .into_response();
    };
    let identity = match state
        .operator()
        .authenticate_session(correlation_id, wire_credential)
        .await
    {
        Ok(identity) => identity,
        Err(error) => return ApiError::from_operator(error, correlation_id).into_response(),
    };
    request.extensions_mut().insert(identity);
    next.run(request).await
}
