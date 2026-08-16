mod cookie;
mod error;
pub(crate) mod handler;
mod middleware;

use std::path::{Path, PathBuf};

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

use crate::{application::enrollment::GatewayIssuer, audit::CorrelationId, db::Database};

use self::error::ApiError;

#[derive(Clone)]
pub(crate) struct AppState {
    database: Database,
    vault_master_key_path: PathBuf,
    gateway_issuer: Option<GatewayIssuer>,
}

/// Builds the mounted Server HTTP surface over an already-migrated database.
pub fn router(database: Database, vault_master_key_path: &Path, web_root: &Path) -> Router {
    router_inner(database, vault_master_key_path, web_root, None)
}

pub(crate) fn router_with_enrollment(
    database: Database,
    vault_master_key_path: &Path,
    web_root: &Path,
    gateway_issuer: GatewayIssuer,
) -> Router {
    router_inner(
        database,
        vault_master_key_path,
        web_root,
        Some(gateway_issuer),
    )
}

fn router_inner(
    database: Database,
    vault_master_key_path: &Path,
    web_root: &Path,
    gateway_issuer: Option<GatewayIssuer>,
) -> Router {
    let state = AppState {
        database,
        vault_master_key_path: vault_master_key_path.to_owned(),
        gateway_issuer,
    };
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
        .merge(handler::session::protected_routes(state.clone()))
        .merge(handler::contest::routes(state.clone()))
        .merge(handler::command::routes(state.clone()))
        .merge(handler::import::routes(state.clone()))
        .merge(handler::provisioning::routes(state.clone()));
    Router::new()
        .merge(handler::health::routes())
        .merge(handler::session::public_routes())
        .merge(handler::enrollment::routes(state.clone()))
        .merge(authenticated)
}

async fn not_found(Extension(correlation_id): Extension<CorrelationId>) -> ApiError {
    ApiError::not_found("unmounted_route", correlation_id)
}

#[cfg(test)]
pub(crate) mod tests;
