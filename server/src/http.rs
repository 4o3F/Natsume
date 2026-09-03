mod cookie;
mod error;
pub(crate) mod handler;
mod middleware;

use std::{path::Path, sync::Arc};

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderValue, header::CACHE_CONTROL},
    middleware as axum_middleware,
    routing::any_service,
};
use tower_http::{
    limit::RequestBodyLimitLayer,
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

use crate::server_state::ServerState;

use self::error::ApiError;

pub(crate) type AppState = Arc<ServerState>;

const API_REQUEST_BODY_LIMIT_BYTES: usize = 4 * 1024 * 1024 + 4 * 1024;

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
        .layer(axum_middleware::from_fn(middleware::request_context))
}

fn api_v2(state: &AppState) -> Router<AppState> {
    let authenticated = Router::new()
        .merge(handler::contest::routes(state.clone()))
        .merge(handler::device::routes(state.clone()))
        .merge(handler::enrollment::routes(state.clone()))
        .merge(handler::import::routes(state.clone()))
        .merge(handler::provisioning::routes(state.clone()))
        .merge(handler::session::protected_routes(state.clone()));
    let public = Router::new()
        .merge(handler::device_control::routes())
        .merge(handler::health::routes())
        .merge(handler::session::public_routes());
    public
        .merge(authenticated)
        .fallback(not_found)
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(API_REQUEST_BODY_LIMIT_BYTES))
        .layer(axum_middleware::from_fn(middleware::reject_head))
}

async fn not_found() -> ApiError {
    ApiError::not_found("unmounted_route")
}

#[cfg(test)]
pub(crate) mod tests;
