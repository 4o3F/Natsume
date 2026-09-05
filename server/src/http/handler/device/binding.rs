use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::component::binding::BindingError;

use super::super::super::{AppState, error::ApiError};
use super::{DevicePath, invalid_device_id, parse_device_id};

#[utoipa::path(
    delete,
    path = "/api/v2/devices/{device_id}/binding",
    operation_id = "deleteDeviceBinding",
    params(DevicePath),
    security(("sessionCookie" = [])),
    responses(
        (status = 204, description = "Device Binding removed or already absent"),
        (status = 400, description = "Invalid Device ID"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Device not found"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn delete_device_binding(
    State(state): State<AppState>,
    Path(path): Path<DevicePath>,
) -> Response {
    let Some(device_id) = parse_device_id(&path) else {
        return invalid_device_id();
    };
    match state.binding().unbind(device_id).await {
        Ok(()) => {
            state.device_control().dirty_device(device_id).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => binding_error(error).into_response(),
    }
}

pub(super) fn binding_error(error: BindingError) -> ApiError {
    match error {
        BindingError::DeviceNotFound => ApiError::not_found("binding_device_not_found"),
        BindingError::DeviceNotEligible => ApiError::conflict("binding_device_not_eligible"),
        BindingError::ConflictingSubmission => ApiError::conflict("binding_submission_conflict"),
        BindingError::InvalidPersistedFacts => {
            ApiError::internal_error("binding_invalid_persisted_facts")
        }
        BindingError::PersistenceFailed => ApiError::internal_error("binding_persistence_failed"),
        BindingError::VaultFailure => ApiError::internal_error("binding_vault_failure"),
    }
}
