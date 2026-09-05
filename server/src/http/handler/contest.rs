use axum::Router;

use super::super::{AppState, middleware};

pub(crate) mod account;
pub(crate) mod binding;
pub(crate) mod seat;

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .merge(seat::routes(state.clone()))
        .merge(account::routes(state.clone()))
        .merge(binding::routes(state))
}
