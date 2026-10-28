use axum::{
    Extension, Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::{
    component::device::{
        DeviceListFilter, DeviceNetworkInfo, DeviceState, EvidenceQuality, LifecycleOutcome,
    },
    device_control::DeviceStatus,
};

use super::super::super::{AppState, error::ApiError};
use super::{
    DevicePath,
    convergence::{DeviceConvergenceResponse, convergence_error},
    invalid_device_id, parse_device_id,
};

/// Durable Device identity and lifecycle with its current complete convergence view.
#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceResponse {
    device_id: String,
    machine_hardware_id: String,
    evidence_quality: DeviceEvidenceQualityResponse,
    state: DeviceStateResponse,
    #[specta(type = specta_typescript::Number)]
    created_at_unix_ms: u64,
    /// Latest authenticated connection addresses, retained after disconnect/restart.
    network: Option<DeviceNetworkResponse>,
    /// Current durable targets and latest validated Actual for this Device.
    convergence: DeviceConvergenceResponse,
}

/// Address diagnostics for the most recently recorded Device control connection.
#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
struct DeviceNetworkResponse {
    /// Client-reported local IP, absent when the latest connection did not report it.
    client_ip: Option<String>,
    /// Canonical socket peer IP observed by the Server, without a port.
    server_observed_ip: String,
    /// Server address-recording time, not a last-heartbeat timestamp.
    #[specta(type = specta_typescript::Number)]
    observed_at_unix_ms: u64,
}

impl From<DeviceNetworkInfo> for DeviceNetworkResponse {
    fn from(network: DeviceNetworkInfo) -> Self {
        Self {
            client_ip: network.client_ip.map(|ip| ip.to_string()),
            server_observed_ip: network.server_observed_ip.to_string(),
            observed_at_unix_ms: network.observed_at_unix_ms,
        }
    }
}

/// Closed Device Enrollment evidence-quality vocabulary exposed by the API.
#[derive(Serialize, specta::Type)]
#[serde(rename_all = "lowercase")]
enum DeviceEvidenceQualityResponse {
    Medium,
    Strong,
}

/// Closed durable Device lifecycle vocabulary exposed by the API.
#[derive(Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "lowercase")]
enum DeviceStateResponse {
    Enabled,
    Disabled,
    Revoked,
}

/// Complete replacement of the mutable Device lifecycle field.
#[derive(Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceUpdateRequest {
    state: DeviceStateResponse,
}

/// `state` is a comma-separated combination of lifecycle values. Omitting it preserves the
/// complete device list.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceListQuery {
    state: Option<String>,
}

#[derive(Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeviceListState {
    Enabled,
    Disabled,
    Revoked,
}

impl From<DeviceListState> for DeviceState {
    fn from(state: DeviceListState) -> Self {
        match state {
            DeviceListState::Enabled => Self::Enabled,
            DeviceListState::Disabled => Self::Disabled,
            DeviceListState::Revoked => Self::Revoked,
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
            network: device.network().map(Into::into),
            convergence: convergence.into(),
        }
    }
}

pub(crate) async fn list_devices(
    State(state): State<AppState>,
    query: Result<Query<DeviceListQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return ApiError::invalid_request("device_list_query_rejected").into_response();
    };
    let filter = query.state.map_or(DeviceListFilter::All, |states| {
        let states = states
            .split(',')
            .map(|state| match state {
                "enabled" => Some(DeviceState::Enabled),
                "disabled" => Some(DeviceState::Disabled),
                "revoked" => Some(DeviceState::Revoked),
                _ => None,
            })
            .collect::<Option<Vec<_>>>();
        DeviceListFilter::States(states.unwrap_or_default())
    });
    if matches!(filter, DeviceListFilter::States(ref states) if states.is_empty()) {
        return ApiError::invalid_request("device_list_query_rejected").into_response();
    }
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

pub(crate) async fn update_device(
    State(state): State<AppState>,
    Extension(admission): Extension<crate::lifecycle::Admission>,
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
                .disable_device(&admission, device_id)
                .await
        }
        DeviceStateResponse::Revoked => {
            state
                .device_control()
                .revoke_device(&admission, device_id)
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

#[cfg(test)]
mod tests;
