use serde::Serialize;
use utoipa::ToSchema;

use super::ConvergenceStatusResponse;
use crate::device_control as model;

/// Redacted Gateway target, Actual, and convergence result.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GatewayConvergenceResponse {
    #[schema(inline)]
    pub(super) status: ConvergenceStatusResponse,
    #[schema(required = true)]
    pub(super) target: Option<GatewayTargetResponse>,
    #[schema(required = true)]
    pub(super) actual: Option<GatewayActualResponse>,
}

impl From<model::GatewayConvergence> for GatewayConvergenceResponse {
    fn from(value: model::GatewayConvergence) -> Self {
        Self {
            status: value.status.into(),
            target: value.target.map(Into::into),
            actual: value.actual.map(Into::into),
        }
    }
}

/// Public fields required to compare the current Gateway target.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GatewayTargetResponse {
    #[schema(
        format = Uuid,
        pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
    )]
    pub(super) credential_id: String,
    #[schema(
        required = true,
        min_length = 64,
        max_length = 64,
        pattern = "^[0-9a-f]{64}$"
    )]
    pub(super) gateway_leaf_sha256: Option<String>,
}

impl From<model::GatewayTarget> for GatewayTargetResponse {
    fn from(value: model::GatewayTarget) -> Self {
        Self {
            credential_id: value.credential_id,
            gateway_leaf_sha256: value.gateway_leaf_sha256,
        }
    }
}

/// Latest validated Gateway Actual reported by the current lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GatewayActualResponse {
    #[schema(
        required = true,
        format = Uuid,
        pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
    )]
    pub(super) credential_id: Option<String>,
    #[schema(inline)]
    pub(super) state: GatewayStateResponse,
    #[schema(
        required = true,
        min_length = 64,
        max_length = 64,
        pattern = "^[0-9a-f]{64}$"
    )]
    pub(super) gateway_leaf_sha256: Option<String>,
}

impl From<model::GatewayActual> for GatewayActualResponse {
    fn from(value: model::GatewayActual) -> Self {
        Self {
            credential_id: value.credential_id,
            state: value.state.into(),
            gateway_leaf_sha256: value.gateway_leaf_sha256,
        }
    }
}

/// Gateway Actual state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum GatewayStateResponse {
    Absent,
    Blocked,
    Restoring,
    Ready,
    UpstreamUnhealthy,
    RecoveryRequired,
}

impl From<model::GatewayState> for GatewayStateResponse {
    fn from(value: model::GatewayState) -> Self {
        match value {
            model::GatewayState::Absent => Self::Absent,
            model::GatewayState::Blocked => Self::Blocked,
            model::GatewayState::Restoring => Self::Restoring,
            model::GatewayState::Ready => Self::Ready,
            model::GatewayState::UpstreamUnhealthy => Self::UpstreamUnhealthy,
            model::GatewayState::RecoveryRequired => Self::RecoveryRequired,
        }
    }
}
