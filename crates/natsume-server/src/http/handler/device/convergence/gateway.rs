use serde::Serialize;

use super::ConvergenceStatusResponse;
use crate::device_control as model;

/// Redacted Gateway target, Actual, and convergence result.
#[derive(PartialEq, Eq, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(super) struct GatewayConvergenceResponse {
    pub(super) status: ConvergenceStatusResponse,
    pub(super) target: Option<GatewayTargetResponse>,
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
#[derive(PartialEq, Eq, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(super) struct GatewayTargetResponse {
    pub(super) credential_id: String,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(super) struct GatewayActualResponse {
    pub(super) credential_id: Option<String>,
    pub(super) state: GatewayStateResponse,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
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
