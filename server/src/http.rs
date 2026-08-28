mod cookie;
mod error;
pub(crate) mod handler;
mod middleware;

use std::{path::Path, sync::Arc};

use axum::{
    Extension, Router,
    http::{HeaderValue, header::CACHE_CONTROL},
    middleware as axum_middleware,
    routing::any_service,
};
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

use crate::{audit::CorrelationId, server_state::ServerState};

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
        .nest("/api/v2", api_v2(&state).fallback(not_found))
        .fallback_service(static_service)
        .with_state(state)
        .layer(axum_middleware::from_fn(middleware::correlation_id))
}

fn api_v2(state: &AppState) -> Router<AppState> {
    let authenticated = Router::new()
        .merge(handler::contest::routes(state.clone()))
        .merge(handler::import::routes(state.clone()))
        .merge(handler::provisioning::routes(state.clone()))
        .merge(handler::session::protected_routes(state.clone()));
    let public = Router::new()
        .merge(handler::health::routes())
        .merge(handler::session::public_routes());
    public.merge(authenticated)
}

async fn not_found(Extension(correlation_id): Extension<CorrelationId>) -> ApiError {
    ApiError::not_found("unmounted_route", correlation_id)
}

#[cfg(test)]
pub(crate) mod tests;
