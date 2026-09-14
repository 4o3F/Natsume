use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{
    component::device::{DeviceListFilter, DeviceState, EvidenceQuality, LifecycleOutcome},
    device_control::DeviceStatus,
};

use super::super::super::{AppState, error::ApiError};
use super::{
    DevicePath,
    convergence::{DeviceConvergenceResponse, convergence_error},
    invalid_device_id, parse_device_id,
};

/// Durable Device identity and lifecycle with its current complete convergence view.
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
    /// Current durable targets and latest validated Actual for this Device.
    convergence: DeviceConvergenceResponse,
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

/// Optional lifecycle filter. Omitting state preserves the complete device list.
#[derive(Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub(crate) struct DeviceListQuery {
    #[param(default = "all")]
    state: Option<DeviceListState>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeviceListState {
    All,
    NonRevoked,
    Enabled,
    Disabled,
    Revoked,
}

impl From<DeviceListState> for DeviceListFilter {
    fn from(state: DeviceListState) -> Self {
        match state {
            DeviceListState::All => Self::All,
            DeviceListState::NonRevoked => Self::NonRevoked,
            DeviceListState::Enabled => Self::State(DeviceState::Enabled),
            DeviceListState::Disabled => Self::State(DeviceState::Disabled),
            DeviceListState::Revoked => Self::State(DeviceState::Revoked),
        }
    }
}

impl From<DeviceStatus> for DeviceResponse {
    fn from(status: DeviceStatus) -> Self {
        let DeviceStatus {
            device,
            convergence,
        } = status;
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
            convergence: convergence.into(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/devices",
    operation_id = "listDevices",
    params(DeviceListQuery),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current durable Devices and complete convergence", body = [DeviceResponse]),
        (status = 400, description = "Invalid device lifecycle filter"),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn list_devices(
    State(state): State<AppState>,
    query: Result<Query<DeviceListQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return ApiError::invalid_request("device_list_query_rejected").into_response();
    };
    let filter = query.state.unwrap_or(DeviceListState::All).into();
    match state.device_control().read_device_statuses(filter).await {
        Ok(statuses) => Json(
            statuses
                .into_iter()
                .map(DeviceResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => convergence_error(error).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/devices/{device_id}",
    operation_id = "getDevice",
    params(DevicePath),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current durable Device and complete convergence", body = DeviceResponse),
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
    match state.device_control().read_device_status(device_id).await {
        Ok(Some(status)) => Json(DeviceResponse::from(status)).into_response(),
        Ok(None) => ApiError::not_found("device_not_found").into_response(),
        Err(error) => convergence_error(error).into_response(),
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
        DeviceStateResponse::Disabled => state.device_control().disable_device(device_id).await,
        DeviceStateResponse::Revoked => state.device_control().revoke_device(device_id).await,
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

#[cfg(test)]
mod tests;
