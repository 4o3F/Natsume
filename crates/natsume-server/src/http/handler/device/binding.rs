use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::delete,
};

use crate::{
    component::binding::{BindingComponent, BindingError},
    device_control::DeviceControl,
    http::{Routes, middleware, middleware::audit::RequestAudit},
};

use super::super::super::error::ApiError;
use super::{DevicePath, invalid_device_id, parse_device_id};

pub(crate) fn operator_routes() -> Routes<DeviceBindingState> {
    vec![(
        "/devices/{device_id}/binding",
        middleware::require_admin(delete(delete_device_binding)),
    )]
}

#[derive(Clone)]
pub(crate) struct DeviceBindingState {
    /// Shared committed binding operations.
    pub(crate) binding: Arc<BindingComponent>,
    /// Shared coordinator for publishing committed changes.
    pub(crate) device_control: Arc<DeviceControl>,
}

pub(in crate::http) async fn delete_device_binding(
    Extension(audit): Extension<RequestAudit>,
    State(state): State<DeviceBindingState>,
    Path(path): Path<DevicePath>,
) -> Response {
    let Some(device_id) = parse_device_id(&path) else {
        return invalid_device_id();
    };
    match state.binding.unbind(device_id, audit.context()).await {
        Ok(change) => {
            state.device_control.publish(change).await;
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
