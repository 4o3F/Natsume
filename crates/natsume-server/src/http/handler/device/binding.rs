use crate::http::middleware::audit::RequestAudit;
use axum::{
    Extension,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::component::binding::BindingError;

use super::super::super::{AppState, error::ApiError};
use super::{DevicePath, invalid_device_id, parse_device_id};

pub(in crate::http) async fn delete_device_binding(
    Extension(audit): Extension<RequestAudit>,
    State(state): State<AppState>,
    Path(path): Path<DevicePath>,
) -> Response {
    let Some(device_id) = parse_device_id(&path) else {
        return invalid_device_id();
    };
    match state.binding().unbind(device_id, audit.context()).await {
        Ok(change) => {
            state.device_control().publish(change).await;
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
        BindingError::InvalidPersistedFacts { .. } => {
            ApiError::internal_error_from(error, "binding_invalid_persisted_facts")
        }
        BindingError::PersistenceFailed { .. } => {
            ApiError::internal_error_from(error, "binding_persistence_failed")
        }
        BindingError::VaultFailure { .. } => {
            ApiError::internal_error_from(error, "binding_vault_failure")
        }
    }
}
