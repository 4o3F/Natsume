use axum::{
    Json,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::component::session::{ForegroundTarget, SessionControlError, SessionControlTarget};

use super::super::super::{AppState, error::ApiError};
use super::{DevicePath, invalid_device_id, parse_device_id};

/// Current durable Session Control target, if it has been initialized.
#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionControlResponse {
    #[schema(required = true)]
    target: Option<SessionControlTargetResponse>,
}

/// Concrete initialized Session Control target.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionControlTargetResponse {
    #[schema(inline)]
    pub(super) foreground_target: ForegroundTargetResponse,
    #[schema(required = true)]
    pub(super) terminate_epoch: Option<u64>,
}

/// Desired Session foreground role accepted and returned by the API.
#[derive(PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(in crate::http::handler) enum ForegroundTargetResponse {
    Contest,
    Waiting,
}

impl From<SessionControlTarget> for SessionControlTargetResponse {
    fn from(target: SessionControlTarget) -> Self {
        Self {
            foreground_target: match target.foreground_target() {
                ForegroundTarget::Contest => ForegroundTargetResponse::Contest,
                ForegroundTarget::Waiting => ForegroundTargetResponse::Waiting,
            },
            terminate_epoch: target.terminate_epoch(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/devices/{device_id}/session-control",
    operation_id = "getDeviceSessionControl",
    params(DevicePath),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current durable Session Control target", body = SessionControlResponse),
        (status = 400, description = "Invalid Device ID"),
        (status = 401, description = "Session authentication failed"),
        (status = 404, description = "Device not found"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn get_session_control(
    State(state): State<AppState>,
    Path(path): Path<DevicePath>,
) -> Response {
    let Some(device_id) = parse_device_id(&path) else {
        return invalid_device_id();
    };
    match state.session().read_current(device_id).await {
        Ok(target) => Json(SessionControlResponse {
            target: target.map(SessionControlTargetResponse::from),
        })
        .into_response(),
        Err(error) => session_error(error).into_response(),
    }
}

pub(super) fn session_error(error: SessionControlError) -> ApiError {
    match error {
        SessionControlError::DeviceNotFound => ApiError::not_found("session_device_not_found"),
        SessionControlError::TerminateEpochOverflow => {
            ApiError::conflict("session_terminate_epoch_exhausted")
        }
        SessionControlError::InvalidPersistedFacts => {
            ApiError::internal_error("session_invalid_persisted_facts")
        }
        SessionControlError::PersistenceFailed => {
            ApiError::internal_error("session_persistence_failed")
        }
    }
}
