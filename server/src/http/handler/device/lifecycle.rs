use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::component::device::{DeviceProjection, DeviceState, EvidenceQuality, LifecycleOutcome};

use super::super::super::{AppState, error::ApiError};
use super::{DevicePath, invalid_device_id, parse_device_id};

/// Durable Device lifecycle and Enrollment evidence shown by the Operator Panel.
#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceResponse {
    device_id: String,
    machine_hardware_id: String,
    #[schema(inline)]
    evidence_quality: DeviceEvidenceQualityResponse,
    #[schema(inline)]
    state: DeviceStateResponse,
    created_at_unix_ms: u64,
}

/// Closed Device Enrollment evidence-quality vocabulary exposed by the API.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum DeviceEvidenceQualityResponse {
    Medium,
    Strong,
}

/// Closed durable Device lifecycle vocabulary exposed by the API.
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum DeviceStateResponse {
    Enabled,
    Disabled,
    Revoked,
}

/// Complete replacement of the mutable Device lifecycle field.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceUpdateRequest {
    #[schema(inline)]
    state: DeviceStateResponse,
}

impl From<DeviceProjection> for DeviceResponse {
    fn from(device: DeviceProjection) -> Self {
        Self {
            device_id: device.device_id().as_text(),
            machine_hardware_id: device.machine_hardware_id().as_text(),
            evidence_quality: match device.evidence_quality() {
                EvidenceQuality::Medium => DeviceEvidenceQualityResponse::Medium,
                EvidenceQuality::Strong => DeviceEvidenceQualityResponse::Strong,
            },
            state: match device.state() {
                DeviceState::Enabled => DeviceStateResponse::Enabled,
                DeviceState::Disabled => DeviceStateResponse::Disabled,
                DeviceState::Revoked => DeviceStateResponse::Revoked,
            },
            created_at_unix_ms: device.created_at_unix_ms(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/devices",
    operation_id = "listDevices",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current durable Devices", body = [DeviceResponse]),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn list_devices(State(state): State<AppState>) -> Response {
    match state.device().list_devices().await {
        Ok(devices) => Json(
            devices
                .into_iter()
                .map(DeviceResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => ApiError::from_device(error).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/devices/{device_id}",
    operation_id = "getDevice",
    params(DevicePath),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current durable Device", body = DeviceResponse),
        (status = 400, description = "Invalid Device ID"),
        (status = 401, description = "Session authentication failed"),
        (status = 404, description = "Device not found"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn get_device(
    State(state): State<AppState>,
    Path(path): Path<DevicePath>,
) -> Response {
    let Some(device_id) = parse_device_id(&path) else {
        return invalid_device_id();
    };
    match state.device().find_device(device_id).await {
        Ok(Some(device)) => Json(DeviceResponse::from(device)).into_response(),
        Ok(None) => ApiError::not_found("device_not_found").into_response(),
        Err(error) => ApiError::from_device(error).into_response(),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v2/devices/{device_id}",
    operation_id = "updateDevice",
    params(DevicePath),
    security(("sessionCookie" = [])),
    request_body = DeviceUpdateRequest,
    responses(
        (status = 204, description = "Device lifecycle updated or already at the requested state"),
        (status = 400, description = "Invalid Device ID or request body"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Device not found"),
        (status = 409, description = "Revoked Device cannot return to a non-terminal state"),
        (status = 413, description = "Request body exceeds the API ingress limit"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn update_device(
    State(state): State<AppState>,
    Path(path): Path<DevicePath>,
    request: Result<Json<DeviceUpdateRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return ApiError::invalid_request("device_update_request_body_rejected").into_response();
    };
    let Some(device_id) = parse_device_id(&path) else {
        return invalid_device_id();
    };
    let outcome = match request.state {
        DeviceStateResponse::Enabled => state.device().enable(device_id).await,
        DeviceStateResponse::Disabled => {
            state
                .device_control()
                .disable_device(Arc::clone(&state), device_id)
                .await
        }
        DeviceStateResponse::Revoked => {
            state
                .device_control()
                .revoke_device(Arc::clone(&state), device_id)
                .await
        }
    };
    match outcome {
        Ok(LifecycleOutcome::Changed | LifecycleOutcome::Unchanged) => {
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(LifecycleOutcome::RejectedTerminal) => {
            ApiError::conflict("device_lifecycle_is_terminal").into_response()
        }
        Err(error) => ApiError::from_device(error).into_response(),
    }
}
