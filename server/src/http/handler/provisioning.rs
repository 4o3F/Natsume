use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::component::provisioning::ProvisioningWindow;

use super::super::{AppState, error::ApiError, middleware};

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    Router::new().route(
        "/provisioning-window",
        middleware::require_operator(state.clone(), get(get_provisioning_window)).merge(
            middleware::require_admin(state, put(update_provisioning_window)),
        ),
    )
}

/// Current process-local provisioning-window state.
#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvisioningWindowResponse {
    #[schema(inline)]
    state: ProvisioningWindowState,
}

/// Complete replacement of the process-local provisioning-window state.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvisioningWindowRequest {
    #[schema(inline)]
    state: ProvisioningWindowState,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum ProvisioningWindowState {
    Closed,
    Open,
}

impl From<ProvisioningWindow> for ProvisioningWindowResponse {
    fn from(window: ProvisioningWindow) -> Self {
        Self {
            state: if window.is_open() {
                ProvisioningWindowState::Open
            } else {
                ProvisioningWindowState::Closed
            },
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
        (status = 401, description = "Session authentication failed")
    )
)]
pub(crate) async fn get_provisioning_window(State(state): State<AppState>) -> Response {
    let window = state.provisioning().read_window().await;
    Json(ProvisioningWindowResponse::from(window)).into_response()
}

#[utoipa::path(
    put,
    path = "/api/v2/provisioning-window",
    operation_id = "updateProvisioningWindow",
    security(("sessionCookie" = [])),
    request_body = ProvisioningWindowRequest,
    responses(
        (status = 200, description = "Provisioning window replaced", body = ProvisioningWindowResponse),
        (status = 400, description = "Invalid request body"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 413, description = "Request body exceeds the API ingress limit")
    )
)]
pub(crate) async fn update_provisioning_window(
    State(state): State<AppState>,
    request: Result<Json<ProvisioningWindowRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return ApiError::invalid_request("provisioning_window_request_body_rejected")
            .into_response();
    };
    let window = match request.state {
        ProvisioningWindowState::Closed => state.provisioning().close_window().await,
        ProvisioningWindowState::Open => state.provisioning().open_window().await,
    };
    Json(ProvisioningWindowResponse::from(window)).into_response()
}
