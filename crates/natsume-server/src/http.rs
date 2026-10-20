mod cookie;
mod error;
pub(crate) mod handler;
mod middleware;

use std::{path::Path, sync::Arc};

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, header::CACHE_CONTROL},
    middleware::{self as axum_middleware, Next},
    response::Response,
    routing::any_service,
};
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

use crate::server_state::ServerState;

use self::error::ApiError;

pub(crate) type AppState = Arc<ServerState>;

/// Builds the mounted Server HTTP surface over process-wide business state.
pub(crate) fn router(state: AppState, web_root: &Path) -> Router {
    let static_service =
        any_service(ServeDir::new(web_root).fallback(ServeFile::new(web_root.join("index.html"))))
            .layer(SetResponseHeaderLayer::overriding(
                CACHE_CONTROL,
                HeaderValue::from_static("no-cache"),
            ));
    Router::new()
        .nest("/api/v2", api_v2(&state))
        .fallback_service(static_service)
        .with_state(state)
        .layer(axum_middleware::from_fn(private_namespace_no_store))
        .layer(axum_middleware::from_fn(middleware::request_context))
}

// Cover root method rejection too, so early responses for this namespace
// have the same cache policy as its handlers.
async fn private_namespace_no_store(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let private = path == "/api/v2/operator"
        || path.starts_with("/api/v2/operator/")
        || path == "/api/v2/print"
        || path.starts_with("/api/v2/print/");
    let mut response = next.run(request).await;
    if private {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn api_v2(state: &AppState) -> Router<AppState> {
    let authenticated = Router::new()
        .merge(handler::contest::routes(state.clone()))
        .merge(handler::device::routes(state.clone()))
        .merge(handler::enrollment::routes(state.clone()))
        .merge(handler::import::routes(state.clone()))
        .merge(handler::provisioning::routes(state.clone()))
        .merge(handler::target_submission::routes(state.clone()))
        .merge(handler::session::protected_routes(state.clone()));
    let public = Router::new()
        .merge(handler::device_control::routes())
        .merge(handler::health::routes())
        .merge(handler::session::public_routes());
    public
        .merge(authenticated)
        .merge(handler::operator::routes(state.clone()))
        .merge(handler::print::routes(state.clone()))
        .fallback(not_found)
        .layer(axum_middleware::from_fn(middleware::reject_head))
}

async fn not_found() -> ApiError {
    ApiError::not_found("unmounted_route")
}

#[cfg(test)]
pub(crate) mod tests;
