use axum::{
    Json, Router,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use utoipa::ToSchema;

pub(in crate::http) fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/health", get(health))
}

#[utoipa::path(
    get,
    path = "/api/v2/health",
    operation_id = "getHealth",
    responses(
        (status = 200, description = "Server is healthy", body = HealthResponse),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn health() -> Response {
    Json(HealthResponse { status: "ok" }).into_response()
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HealthResponse {
    status: &'static str,
}
