use axum::{
    Router,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::audit::CorrelationId;

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

fn current_facts_response<T: Serialize>(facts: &[T], correlation_id: CorrelationId) -> Response {
    let body = serde_json::to_string(&facts).unwrap_or_else(|_| {
        tracing::error!(
            correlation_id = %correlation_id.as_text(),
            "current facts response serialization invariant failed"
        );
        panic!("current facts response serialization invariant failed");
    });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}
