pub(in crate::http) mod audit;
mod authentication;
mod completion;
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

pub(super) use authentication::{authenticate_client, authenticate_operator, authenticate_source};
pub(super) use completion::{CompletionState, complete_mutation};
pub(super) use request_context::request_context;

pub(super) async fn reject_head(request: Request, next: Next) -> Response {
    if request.method() == Method::HEAD {
        let mut response = ApiError::not_found("unmounted_route").into_response();
        *response.body_mut() = Body::empty();
        return response;
    }
    next.run(request).await
}

pub(in crate::http) fn require_admin(routes: MethodRouter<AppState>) -> MethodRouter<AppState> {
    routes.route_layer(axum_middleware::from_fn(require_admin_role))
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
