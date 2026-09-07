use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::component::session::{ForegroundTarget, SessionControlError, SessionControlTarget};

use super::super::super::{AppState, error::ApiError};
use super::{DevicePath, invalid_device_id, parse_device_id};

/// Current durable Session Control target, if it has been initialized.
#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionControlResponse {
    #[schema(required = true)]
    target: Option<SessionControlTargetResponse>,
}

/// Concrete initialized Session Control target.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionControlTargetResponse {
    #[schema(inline)]
    pub(super) foreground_target: ForegroundTargetResponse,
    #[schema(required = true)]
    pub(super) terminate_epoch: Option<u64>,
}

/// Desired Session foreground role accepted and returned by the API.
#[derive(PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum ForegroundTargetResponse {
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

/// Complete Session foreground mutation body.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionForegroundRequest {
    #[schema(inline)]
    foreground_target: ForegroundTargetResponse,
}

#[utoipa::path(
    get,
    path = "/api/v2/devices/{device_id}/session-control",
    operation_id = "getDeviceSessionControl",
    params(DevicePath),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current durable Session Control target", body = SessionControlResponse),
        (status = 400, description = "Invalid Device ID"),
        (status = 401, description = "Session authentication failed"),
        (status = 404, description = "Device not found"),
        (status = 500, description = "Internal failure")
    )
)]
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

#[utoipa::path(
    put,
    path = "/api/v2/devices/{device_id}/session-control",
    operation_id = "setDeviceSessionForeground",
    params(DevicePath),
    security(("sessionCookie" = [])),
    request_body = SessionForegroundRequest,
    responses(
        (status = 200, description = "Session foreground target committed", body = SessionControlResponse),
        (status = 400, description = "Invalid Device ID or request body"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Device not found"),
        (status = 413, description = "Request body exceeds the API ingress limit"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn set_session_foreground(
    State(state): State<AppState>,
    Path(path): Path<DevicePath>,
    request: Result<Json<SessionForegroundRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return ApiError::invalid_request("session_control_request_body_rejected").into_response();
    };
    let Some(device_id) = parse_device_id(&path) else {
        return invalid_device_id();
    };
    let foreground_target = match request.foreground_target {
        ForegroundTargetResponse::Contest => ForegroundTarget::Contest,
        ForegroundTargetResponse::Waiting => ForegroundTarget::Waiting,
    };
    match state
        .session()
        .set_foreground(device_id, foreground_target)
        .await
    {
        Ok(target) => {
            state.device_control().dirty_device(device_id).await;
            Json(SessionControlResponse {
                target: Some(SessionControlTargetResponse::from(target)),
            })
            .into_response()
        }
        Err(error) => session_error(error).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/devices/{device_id}/session-control/actions/terminate",
    operation_id = "terminateDeviceSession",
    params(DevicePath),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Session terminate epoch advanced", body = SessionControlResponse),
        (status = 400, description = "Invalid Device ID"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Device not found"),
        (status = 409, description = "Terminate epoch exhausted"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn terminate_session(
    State(state): State<AppState>,
    Path(path): Path<DevicePath>,
) -> Response {
    let Some(device_id) = parse_device_id(&path) else {
        return invalid_device_id();
    };
    match state.session().terminate(device_id).await {
        Ok(target) => {
            state.device_control().dirty_device(device_id).await;
            Json(SessionControlResponse {
                target: Some(SessionControlTargetResponse::from(target)),
            })
            .into_response()
        }
        Err(error) => session_error(error).into_response(),
    }
}

pub(super) fn session_error(error: SessionControlError) -> ApiError {
    match error {
        SessionControlError::DeviceNotFound => ApiError::not_found("session_device_not_found"),
        SessionControlError::TerminateEpochOverflow => {
            ApiError::conflict("session_terminate_epoch_exhausted")
        }
        SessionControlError::InvalidPersistedFacts => {
            ApiError::internal_error("session_invalid_persisted_facts")
        }
        SessionControlError::PersistenceFailed => {
            ApiError::internal_error("session_persistence_failed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ForegroundTargetResponse, SessionForegroundRequest};

    #[test]
    fn foreground_requests_accept_only_managed_roles_and_reject_the_old_contract() {
        for (role, expected) in [
            ("waiting", ForegroundTargetResponse::Waiting),
            ("contest", ForegroundTargetResponse::Contest),
        ] {
            let parsed = serde_json::from_value::<SessionForegroundRequest>(
                serde_json::json!({"foreground_target": role}),
            )
            .unwrap_or_else(|error| panic!("valid foreground: {error}"));
            assert!(parsed.foreground_target == expected);
        }
        for invalid in [
            serde_json::json!({"lock_state": "locked"}),
            serde_json::json!({"lock_state": "unlocked"}),
            serde_json::json!({"foreground_target": "locked"}),
            serde_json::json!({"foreground_target": "unlocked"}),
            serde_json::json!({"foreground_target": "greeter"}),
            serde_json::json!({"foreground_target": "other"}),
            serde_json::json!({"foreground_target": "none"}),
            serde_json::json!({"foreground_target": "unknown"}),
            serde_json::json!({"foreground_target": null}),
            serde_json::json!({"foreground_target": "waiting", "terminate_epoch": 1}),
            serde_json::json!({"foreground_target": "contest", "lock_state": "unlocked"}),
            serde_json::json!({}),
        ] {
            assert!(serde_json::from_value::<SessionForegroundRequest>(invalid).is_err());
        }
    }
}
