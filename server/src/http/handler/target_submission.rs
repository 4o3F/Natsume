use axum::{
    Extension, Json, Router,
    extract::{State, rejection::JsonRejection},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::component::{
    device::DeviceId,
    operator::OperatorIdentity,
    session::ForegroundTarget,
    target_submission::{
        TargetAction, TargetRejection, TargetScope, TargetSubmissionError, TargetSubmissionRequest,
        TargetSubmissionResult,
    },
};

use super::{
    super::{AppState, error::ApiError, middleware},
    device::session::ForegroundTargetResponse,
};

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    Router::new().route(
        "/target-submissions",
        middleware::require_admin(state, post(submit_targets)),
    )
}

/// One durable, replayable Server target submission. No Client execution is awaited.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TargetSubmissionBody {
    #[schema(format = "uuid")]
    operation_id: String,
    scope: TargetScopeBody,
    action: TargetActionBody,
}

#[derive(Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TargetScopeBody {
    AllEnabled {},
    AllOnlineEnabled {},
    Devices {
        #[schema(min_items = 1)]
        device_ids: Vec<String>,
    },
}

#[derive(Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TargetActionBody {
    SetForeground {
        foreground_target: ForegroundTargetResponse,
    },
    TerminateSession {},
    ResetHome {},
    PowerOff {},
}

impl TargetSubmissionBody {
    #[cfg(test)]
    fn into_request(self) -> Option<TargetSubmissionRequest> {
        self.into_request_with(Vec::new(), 1)
    }

    fn into_request_with(
        self,
        active_devices: Vec<DeviceId>,
        expires_at_unix_ms: i64,
    ) -> Option<TargetSubmissionRequest> {
        let operation_id = Uuid::parse_str(&self.operation_id).ok()?;
        if operation_id.is_nil() || operation_id.hyphenated().to_string() != self.operation_id {
            return None;
        }
        let scope = match self.scope {
            TargetScopeBody::AllEnabled {} => TargetScope::AllEnabled,
            TargetScopeBody::AllOnlineEnabled {} => TargetScope::AllOnlineEnabled(active_devices),
            TargetScopeBody::Devices { device_ids } => {
                if device_ids.is_empty() {
                    return None;
                }
                TargetScope::Devices(
                    device_ids
                        .iter()
                        .map(|id| DeviceId::parse(id))
                        .collect::<Option<_>>()?,
                )
            }
        };
        let action = match self.action {
            TargetActionBody::SetForeground { foreground_target } => {
                TargetAction::SetForeground(match foreground_target {
                    ForegroundTargetResponse::Waiting => ForegroundTarget::Waiting,
                    ForegroundTargetResponse::Contest => ForegroundTarget::Contest,
                })
            }
            TargetActionBody::TerminateSession {} => TargetAction::TerminateSession,
            TargetActionBody::ResetHome {} => TargetAction::ResetHome,
            TargetActionBody::PowerOff {} => TargetAction::PowerOff { expires_at_unix_ms },
        };
        if matches!(action, TargetAction::PowerOff { .. })
            != matches!(scope, TargetScope::AllOnlineEnabled(_))
        {
            return None;
        }
        Some(TargetSubmissionRequest {
            operation_id,
            scope,
            action,
        })
    }
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TargetSubmissionResponse {
    #[schema(format = "uuid")]
    operation_id: String,
    results: Vec<DeviceSubmissionResponse>,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DeviceSubmissionResponse {
    Submitted {
        device_id: String,
    },
    Rejected {
        device_id: String,
        code: TargetRejectionCode,
        message: &'static str,
    },
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TargetRejectionCode {
    DeviceNotFound,
    DeviceNotEnabled,
    InvalidDeviceState,
    InvalidTarget,
    EpochExhausted,
}

impl From<TargetSubmissionResult> for TargetSubmissionResponse {
    fn from(result: TargetSubmissionResult) -> Self {
        Self {
            operation_id: result.operation_id.hyphenated().to_string(),
            results: result
                .results
                .into_iter()
                .map(|row| {
                    let device_id = row.device_id.as_text();
                    let Some(rejection) = row.rejection else {
                        return DeviceSubmissionResponse::Submitted { device_id };
                    };
                    let (code, message) = match rejection {
                        TargetRejection::DeviceNotFound => {
                            (TargetRejectionCode::DeviceNotFound, "Device not found")
                        }
                        TargetRejection::DeviceNotEnabled => (
                            TargetRejectionCode::DeviceNotEnabled,
                            "Device is not enabled",
                        ),
                        TargetRejection::InvalidDeviceState => (
                            TargetRejectionCode::InvalidDeviceState,
                            "Device state is invalid",
                        ),
                        TargetRejection::InvalidTarget => (
                            TargetRejectionCode::InvalidTarget,
                            "Stored target is invalid",
                        ),
                        TargetRejection::EpochExhausted => (
                            TargetRejectionCode::EpochExhausted,
                            "Target epoch is exhausted",
                        ),
                    };
                    DeviceSubmissionResponse::Rejected {
                        device_id,
                        code,
                        message,
                    }
                })
                .collect(),
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/target-submissions",
    operation_id = "submitTargets",
    security(("sessionCookie" = [])),
    request_body = TargetSubmissionBody,
    responses(
        (status = 200, description = "Durable per-device submission results, including a replay of an existing operation ID", body = TargetSubmissionResponse),
        (status = 400, description = "Invalid submission, action, scope or canonical UUID"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 409, description = "Operation ID belongs to another request or operator"),
        (status = 413, description = "Request body exceeds the API ingress limit"),
        (status = 500, description = "Submission transaction failed")
    )
)]
pub(crate) async fn submit_targets(
    State(state): State<AppState>,
    Extension(identity): Extension<OperatorIdentity>,
    body: Result<Json<TargetSubmissionBody>, JsonRejection>,
) -> Response {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_millis()).ok());
    let Some(expires_at_unix_ms) = now.and_then(|value| value.checked_add(60_000)) else {
        return ApiError::internal_error("target_submission_clock_unavailable").into_response();
    };
    let active_devices = state.device_control().active_device_ids().await;
    let Some(request) = body
        .ok()
        .and_then(|Json(body)| body.into_request_with(active_devices, expires_at_unix_ms))
    else {
        return ApiError::invalid_request("target_submission_body_rejected").into_response();
    };
    match state
        .target_submission()
        .submit(identity.operator_id(), request)
        .await
    {
        Ok(result) => {
            let devices = result
                .results
                .iter()
                .filter(|row| row.rejection.is_none())
                .map(|row| row.device_id)
                .collect::<Vec<_>>();
            state.device_control().dirty_devices(&devices).await;
            Json(TargetSubmissionResponse::from(result)).into_response()
        }
        Err(TargetSubmissionError::OperationIdConflict) => {
            ApiError::conflict("target_submission_id_conflict").into_response()
        }
        Err(TargetSubmissionError::InvalidPersistedFacts) => {
            ApiError::internal_error("target_submission_invalid_persisted_facts").into_response()
        }
        Err(TargetSubmissionError::PersistenceFailed) => {
            ApiError::internal_error("target_submission_persistence_failed").into_response()
        }
    }
}

#[cfg(test)]
mod tests;
