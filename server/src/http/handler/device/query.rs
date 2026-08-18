use axum::{
    Extension, Router,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    application::{
        device::{self, DeviceFacts, DeviceState, HardwareIdentityQuality},
        operator::OperatorIdentity,
    },
    audit::CorrelationId,
};

use super::super::super::{AppState, error::ApiError, middleware};

pub(super) fn routes(state: AppState) -> Router<AppState> {
    Router::new().route("/devices", middleware::operator_get(state, list_devices))
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceResponse {
    device_id: String,
    #[schema(inline)]
    state: DeviceResponseState,
    #[schema(inline)]
    hardware_identity_quality: DeviceResponseHardwareIdentityQuality,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum DeviceResponseState {
    Enrolled,
    Revoked,
    Disabled,
}

impl From<DeviceState> for DeviceResponseState {
    fn from(state: DeviceState) -> Self {
        match state {
            DeviceState::Enrolled => Self::Enrolled,
            DeviceState::Revoked => Self::Revoked,
            DeviceState::Disabled => Self::Disabled,
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum DeviceResponseHardwareIdentityQuality {
    Strong,
    Medium,
    Weak,
}

impl From<HardwareIdentityQuality> for DeviceResponseHardwareIdentityQuality {
    fn from(quality: HardwareIdentityQuality) -> Self {
        match quality {
            HardwareIdentityQuality::Strong => Self::Strong,
            HardwareIdentityQuality::Medium => Self::Medium,
            HardwareIdentityQuality::Weak => Self::Weak,
        }
    }
}

impl From<DeviceFacts> for DeviceResponse {
    fn from(facts: DeviceFacts) -> Self {
        let (device_id, state, hardware_identity_quality) = facts.into_parts();
        Self {
            device_id: device_id.as_text(),
            state: state.into(),
            hardware_identity_quality: hardware_identity_quality.into(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/devices",
    operation_id = "listDevices",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current Device set", body = [DeviceResponse]),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn list_devices(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(_identity): Extension<OperatorIdentity>,
) -> Response {
    match device::list_devices(&state.database).await {
        Ok(facts) => current_facts_response(
            &facts
                .into_iter()
                .map(DeviceResponse::from)
                .collect::<Vec<_>>(),
            correlation_id,
        ),
        Err(error) => ApiError::from_device(error, correlation_id).into_response(),
    }
}

fn current_facts_response(facts: &[DeviceResponse], correlation_id: CorrelationId) -> Response {
    let body = serde_json::to_string(&facts).unwrap_or_else(|_| {
        tracing::error!(
            correlation_id = %correlation_id.as_text(),
            "current facts response serialization invariant failed"
        );
        panic!("current facts response serialization invariant failed");
    });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}
