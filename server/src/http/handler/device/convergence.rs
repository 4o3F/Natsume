use axum::{
    Json,
    extract::{Path, State},
    response::{IntoResponse, Response},
};

use crate::{
    component::{gateway::GatewayError, runtime::RuntimeConfigError},
    device_control::DeviceConvergenceError,
};

pub(crate) use crate::device_control::DeviceConvergenceResponse;

use super::super::super::{AppState, error::ApiError};
use super::{
    DevicePath, binding::binding_error, home::home_error, invalid_device_id, parse_device_id,
    session::session_error,
};

#[utoipa::path(
    get,
    path = "/api/v2/devices/{device_id}/convergence",
    operation_id = "getDeviceConvergence",
    params(DevicePath),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current redacted Server targets and latest valid Client Actual", body = DeviceConvergenceResponse),
        (status = 400, description = "Invalid Device ID"),
        (status = 401, description = "Session authentication failed"),
        (status = 404, description = "Device not found"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn get_device_convergence(
    State(state): State<AppState>,
    Path(path): Path<DevicePath>,
) -> Result<Json<DeviceConvergenceResponse>, Response> {
    let device_id = parse_device_id(&path).ok_or_else(invalid_device_id)?;
    let convergence = state
        .device_control()
        .read_convergence(&state, device_id)
        .await
        .map_err(|error| convergence_error(error).into_response())?
        .ok_or_else(|| ApiError::not_found("device_not_found").into_response())?;
    Ok(Json(convergence))
}

fn convergence_error(error: DeviceConvergenceError) -> ApiError {
    match error {
        DeviceConvergenceError::Device(error) => ApiError::from_device(error),
        DeviceConvergenceError::Gateway(error) => gateway_error(error),
        DeviceConvergenceError::Binding(error) => binding_error(error),
        DeviceConvergenceError::Runtime(error) => runtime_error(error),
        DeviceConvergenceError::Session(error) => session_error(error),
        DeviceConvergenceError::Home(error) => home_error(error),
    }
}

fn gateway_error(error: GatewayError) -> ApiError {
    match error {
        GatewayError::InvalidCsr => ApiError::internal_error("gateway_invalid_persisted_csr"),
        GatewayError::ConflictingCsr => {
            ApiError::internal_error("gateway_conflicting_persisted_csr")
        }
        GatewayError::InvalidPersistedFacts => {
            ApiError::internal_error("gateway_invalid_persisted_facts")
        }
        GatewayError::PersistenceFailed => ApiError::internal_error("gateway_persistence_failed"),
        GatewayError::IssuanceFailed => ApiError::internal_error("gateway_issuance_failed"),
    }
}

fn runtime_error(error: RuntimeConfigError) -> ApiError {
    match error {
        RuntimeConfigError::MissingConfiguration => {
            ApiError::internal_error("runtime_config_missing_configuration")
        }
        RuntimeConfigError::InvalidPersistedFacts => {
            ApiError::internal_error("runtime_config_invalid_persisted_facts")
        }
        RuntimeConfigError::PersistenceFailed => {
            ApiError::internal_error("runtime_config_persistence_failed")
        }
    }
}
