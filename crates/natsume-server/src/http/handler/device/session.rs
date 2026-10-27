use axum::{
    Json,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::component::session::{ForegroundTarget, SessionControlError, SessionControlTarget};

use super::super::super::{AppState, error::ApiError};
use super::{DevicePath, invalid_device_id, parse_device_id};

/// Current durable Session Control target, if it has been initialized.
#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionControlResponse {
    target: Option<SessionControlTargetResponse>,
}

/// Concrete initialized Session Control target.
#[derive(PartialEq, Eq, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionControlTargetResponse {
    pub(super) foreground_target: ForegroundTargetResponse,
    #[specta(type = Option<specta_typescript::Number>)]
    pub(super) terminate_epoch: Option<u64>,
}

/// Desired Session foreground role accepted and returned by the API.
#[derive(PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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
        SessionControlError::InvalidPersistedFacts { .. } => {
            ApiError::internal_error_from(error, "session_invalid_persisted_facts")
        }
        SessionControlError::PersistenceFailed { .. } => {
            ApiError::internal_error_from(error, "session_persistence_failed")
        }
    }
}
