use axum::{
    Extension, Router,
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::audit::CorrelationId;

pub(in crate::http) fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/health", get(health).head(super::super::not_found))
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
pub(crate) async fn health(Extension(correlation_id): Extension<CorrelationId>) -> Response {
    let body = serde_json::to_string(&HealthResponse { status: "ok" }).unwrap_or_else(|_| {
        tracing::error!(
            correlation_id = %correlation_id.as_text(),
            "health response serialization invariant failed"
        );
        panic!("health response serialization invariant failed");
    });
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HealthResponse {
    status: &'static str,
}
