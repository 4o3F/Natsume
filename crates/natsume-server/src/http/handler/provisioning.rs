use crate::http::Routes;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use serde::{Deserialize, Serialize};

use crate::component::provisioning::ProvisioningWindow;

use super::super::{AppState, error::ApiError, middleware};

pub(in crate::http) fn operator_routes() -> Routes {
    vec![(
        "/provisioning-window",
        get(get_provisioning_window)
            .merge(middleware::require_admin(put(update_provisioning_window))),
    )]
}

/// Current process-local automatic Enrollment approval state.
#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvisioningWindowResponse {
    state: ProvisioningWindowState,
}

/// Open automatically approves new and pending enrollments; closed requires administrator approval.
#[derive(Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvisioningWindowRequest {
    state: ProvisioningWindowState,
}

#[derive(Deserialize, Serialize, specta::Type)]
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

pub(crate) async fn get_provisioning_window(State(state): State<AppState>) -> Response {
    let window = state.provisioning().read_window();
    Json(ProvisioningWindowResponse::from(window)).into_response()
}

pub(crate) async fn update_provisioning_window(
    State(state): State<AppState>,
    request: Result<Json<ProvisioningWindowRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return ApiError::invalid_request("provisioning_window_request_body_rejected")
            .into_response();
    };
    let window = match request.state {
        ProvisioningWindowState::Closed => state.provisioning().close_window(),
        ProvisioningWindowState::Open => state.provisioning().open_window(),
    };
    Json(ProvisioningWindowResponse::from(window)).into_response()
}
