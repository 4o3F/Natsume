use axum::{
    Json,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::component::home::HomeError;

use super::super::super::{AppState, error::ApiError};
use super::{DevicePath, invalid_device_id, parse_device_id};

/// Current durable Home target, if any reset has been requested.
/// Process-local Device connection state at query time.
#[derive(PartialEq, Eq, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct HomeResponse {
    #[specta(type = Option<specta_typescript::Number>)]
    reset_epoch: Option<u64>,
}

pub(crate) async fn get_home(
    State(state): State<AppState>,
    Path(path): Path<DevicePath>,
) -> Response {
    let Some(device_id) = parse_device_id(&path) else {
        return invalid_device_id();
    };
    match state.home().read_current(device_id).await {
        Ok(reset_epoch) => Json(HomeResponse { reset_epoch }).into_response(),
        Err(error) => home_error(error).into_response(),
    }
}

pub(super) fn home_error(error: HomeError) -> ApiError {
    match error {
        HomeError::DeviceNotFound => ApiError::not_found("home_device_not_found"),
        HomeError::EpochExhausted => ApiError::conflict("home_reset_epoch_exhausted"),
        HomeError::InvalidPersistedFacts { .. } => {
            ApiError::internal_error_from(error, "home_invalid_persisted_facts")
        }
        HomeError::PersistenceFailed { .. } => {
            ApiError::internal_error_from(error, "home_persistence_failed")
        }
    }
}
