use axum::{
    Json, Router,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

pub(in crate::http) fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/health", get(health))
}

pub(crate) async fn health() -> Response {
    Json(HealthResponse { status: "ok" }).into_response()
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct HealthResponse {
    status: &'static str,
}
