use axum::{
    Extension, Json, Router,
    extract::{Request, State},
    middleware as axum_middleware,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    application::{
        operator::{self, OperatorIdentity},
        provisioning::{self, ProvisioningWindow, ProvisioningWindowState},
    },
    audit::CorrelationId,
};

use super::super::{AppState, error::ApiError, middleware};

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    let open =
        post(open_provisioning_window).route_layer(axum_middleware::from_fn(require_admin_role));
    let close =
        post(close_provisioning_window).route_layer(axum_middleware::from_fn(require_admin_role));
    Router::new()
        .route(
            "/provisioning-window",
            middleware::operator_get(state.clone(), get_provisioning_window),
        )
        .route(
            "/provisioning-window/actions/open",
            middleware::require_operator(state.clone(), open),
        )
        .route(
            "/provisioning-window/actions/close",
            middleware::require_operator(state, close),
        )
}

async fn require_admin_role(
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(identity): Extension<OperatorIdentity>,
    request: Request,
    next: Next,
) -> Response {
    if let Err(error) = operator::require_admin(identity.role()) {
        return ApiError::from_operator(error, correlation_id).into_response();
    }
    next.run(request).await
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvisioningWindowResponse {
    #[schema(inline)]
    state: ProvisioningWindowResponseState,
    revision: i64,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum ProvisioningWindowResponseState {
    Closed,
    Open,
}

impl From<ProvisioningWindowState> for ProvisioningWindowResponseState {
    fn from(state: ProvisioningWindowState) -> Self {
        match state {
            ProvisioningWindowState::Closed => Self::Closed,
            ProvisioningWindowState::Open => Self::Open,
        }
    }
}

impl From<ProvisioningWindow> for ProvisioningWindowResponse {
    fn from(window: ProvisioningWindow) -> Self {
        Self {
            state: ProvisioningWindowResponseState::from(window.state),
            revision: window.revision,
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/provisioning-window",
    operation_id = "getProvisioningWindow",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current provisioning-window fact", body = ProvisioningWindowResponse),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn get_provisioning_window(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
) -> Response {
    match provisioning::read_window(&state.database).await {
        Ok(window) => Json(ProvisioningWindowResponse::from(window)).into_response(),
        Err(error) => ApiError::from_provisioning(error, correlation_id).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/provisioning-window/actions/open",
    operation_id = "openProvisioningWindow",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Provisioning window opened or already open", body = ProvisioningWindowResponse),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn open_provisioning_window(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
) -> Response {
    match provisioning::open_window(&state.database, correlation_id).await {
        Ok(window) => Json(ProvisioningWindowResponse::from(window)).into_response(),
        Err(error) => ApiError::from_provisioning(error, correlation_id).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/provisioning-window/actions/close",
    operation_id = "closeProvisioningWindow",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Provisioning window closed or already closed", body = ProvisioningWindowResponse),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn close_provisioning_window(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
) -> Response {
    match provisioning::close_window(&state.database, correlation_id).await {
        Ok(window) => Json(ProvisioningWindowResponse::from(window)).into_response(),
        Err(error) => ApiError::from_provisioning(error, correlation_id).into_response(),
    }
}

#[cfg(test)]
#[path = "provisioning/tests.rs"]
mod tests;
