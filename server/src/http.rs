mod cookie;
mod error;
pub(crate) mod handler;
mod middleware;

use axum::{Extension, Router, middleware as axum_middleware};

use crate::{audit::CorrelationId, db::Database};

use self::error::ApiError;

#[derive(Clone)]
pub(crate) struct AppState {
    database: Database,
}

/// Builds the mounted Server HTTP surface over an already-migrated database.
pub fn router(database: Database) -> Router {
    let state = AppState { database };
    Router::new()
        .nest("/api/v2", api_v2(state.clone()))
        .fallback(not_found)
        .with_state(state)
        .layer(axum_middleware::from_fn(middleware::correlation_id))
}

fn api_v2(state: AppState) -> Router<AppState> {
    let authenticated = Router::new()
        .merge(handler::session::protected_routes(state.clone()))
        .merge(handler::contest::routes(state));
    Router::new()
        .merge(handler::health::routes())
        .merge(handler::session::public_routes())
        .merge(authenticated)
}

async fn not_found(Extension(correlation_id): Extension<CorrelationId>) -> ApiError {
    ApiError::not_found("unmounted_route", correlation_id)
}

#[cfg(test)]
pub(crate) mod tests;
