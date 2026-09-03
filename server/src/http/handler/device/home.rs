use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::component::home::HomeError;

use super::super::super::{AppState, error::ApiError};
use super::{DevicePath, invalid_device_id, parse_device_id};

/// Current durable Home target, if any reset has been requested.
/// Process-local Device connection state at query time.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HomeResponse {
    #[schema(required = true)]
    reset_epoch: Option<u64>,
}

#[utoipa::path(
    get,
    path = "/api/v2/devices/{device_id}/home",
    operation_id = "getDeviceHome",
    params(DevicePath),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current durable Home target", body = HomeResponse),
        (status = 400, description = "Invalid Device ID"),
        (status = 401, description = "Session authentication failed"),
        (status = 404, description = "Device not found"),
        (status = 500, description = "Internal failure")
    )
)]
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

#[utoipa::path(
    post,
    path = "/api/v2/devices/{device_id}/home/actions/reset",
    operation_id = "resetDeviceHome",
    params(DevicePath),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Home reset epoch advanced", body = HomeResponse),
        (status = 400, description = "Invalid Device ID"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Device not found"),
        (status = 409, description = "Home reset epoch exhausted"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn reset_home(
    State(state): State<AppState>,
    Path(path): Path<DevicePath>,
) -> Response {
    let Some(device_id) = parse_device_id(&path) else {
        return invalid_device_id();
    };
    match state.home().reset(device_id).await {
        Ok(reset_epoch) => {
            state
                .device_control()
                .dirty_one(Arc::clone(&state), device_id)
                .await;
            Json(HomeResponse {
                reset_epoch: Some(reset_epoch),
            })
            .into_response()
        }
        Err(error) => home_error(error).into_response(),
    }
}

pub(super) fn home_error(error: HomeError) -> ApiError {
    match error {
        HomeError::DeviceNotFound => ApiError::not_found("home_device_not_found"),
        HomeError::EpochExhausted => ApiError::conflict("home_reset_epoch_exhausted"),
        HomeError::InvalidPersistedFacts => {
            ApiError::internal_error("home_invalid_persisted_facts")
        }
        HomeError::PersistenceFailed => ApiError::internal_error("home_persistence_failed"),
    }
}
