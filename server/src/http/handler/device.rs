use axum::Router;

use super::super::AppState;

pub(crate) mod enrollment;
pub(crate) mod lifecycle;
pub(crate) mod query;

pub(in crate::http) fn public_routes(state: AppState) -> Router<AppState> {
    enrollment::public_routes(state)
}

pub(in crate::http) fn protected_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .merge(query::routes(state.clone()))
        .merge(lifecycle::routes(state.clone()))
        .merge(enrollment::protected_routes(state))
}
