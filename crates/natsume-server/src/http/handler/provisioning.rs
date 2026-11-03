use std::sync::Arc;

use crate::http::Routes;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use serde::{Deserialize, Serialize};

use crate::component::provisioning::{ProvisioningComponent, ProvisioningWindow};

use super::super::{error::ApiError, middleware};

pub(crate) fn operator_routes() -> Routes<Arc<ProvisioningComponent>> {
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

pub(crate) async fn get_provisioning_window(
    State(state): State<Arc<ProvisioningComponent>>,
) -> Response {
    let window = state.read_window();
    Json(ProvisioningWindowResponse::from(window)).into_response()
}

pub(crate) async fn update_provisioning_window(
    State(state): State<Arc<ProvisioningComponent>>,
    request: Result<Json<ProvisioningWindowRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return ApiError::invalid_request("provisioning_window_request_body_rejected")
            .into_response();
    };
    let window = match request.state {
        ProvisioningWindowState::Closed => state.close_window(),
        ProvisioningWindowState::Open => state.open_window(),
    };
    Json(ProvisioningWindowResponse::from(window)).into_response()
}
