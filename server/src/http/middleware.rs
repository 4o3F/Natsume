mod authentication;
mod request_context;

use axum::{
    handler::Handler,
    middleware as axum_middleware,
    routing::{MethodRouter, get},
};

use super::{AppState, not_found};

pub(super) use request_context::request_context;

pub(in crate::http) fn require_operator(
    state: AppState,
    routes: MethodRouter<AppState>,
) -> MethodRouter<AppState> {
    routes.route_layer(axum_middleware::from_fn_with_state(
        state,
        authentication::authenticate_operator,
    ))
}

/// Builds an operator-authenticated GET route. HEAD is answered with the
/// uniform not-found response so the method-level bypass in
/// `authenticate_operator` can never reach a real handler. Routing protected
/// GETs through this helper keeps that invariant structural instead of a
/// per-callsite convention that `get()`'s implicit HEAD support would break.
pub(in crate::http) fn operator_get<H, T>(state: AppState, handler: H) -> MethodRouter<AppState>
where
    H: Handler<T, AppState>,
    T: 'static,
{
    require_operator(state, get(handler).head(not_found))
}
