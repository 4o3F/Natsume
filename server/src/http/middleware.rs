mod authentication;
mod request_context;

use axum::{
    Extension,
    body::Body,
    extract::Request,
    http::Method,
    middleware as axum_middleware,
    middleware::Next,
    response::{IntoResponse as _, Response},
    routing::MethodRouter,
};

use crate::component::operator::OperatorIdentity;

use super::{AppState, error::ApiError};

pub(super) use request_context::request_context;

pub(super) async fn reject_head(request: Request, next: Next) -> Response {
    if request.method() == Method::HEAD {
        let mut response = ApiError::not_found("unmounted_route").into_response();
        *response.body_mut() = Body::empty();
        return response;
    }
    next.run(request).await
}

pub(in crate::http) fn require_operator(
    state: AppState,
    routes: MethodRouter<AppState>,
) -> MethodRouter<AppState> {
    routes.route_layer(axum_middleware::from_fn_with_state(
        state,
        authentication::authenticate_operator,
    ))
}

/// Protects routes with operator authentication and the Administrator role.
pub(in crate::http) fn require_admin(
    state: AppState,
    routes: MethodRouter<AppState>,
) -> MethodRouter<AppState> {
    require_operator(
        state,
        routes.route_layer(axum_middleware::from_fn(require_admin_role)),
    )
}

async fn require_admin_role(
    Extension(identity): Extension<OperatorIdentity>,
    request: Request,
    next: Next,
) -> Response {
    if let Err(error) = identity.require_admin() {
        return ApiError::from_operator(error).into_response();
    }
    next.run(request).await
}
