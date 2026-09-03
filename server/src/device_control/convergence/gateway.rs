use serde::Serialize;
use sha2::{Digest as _, Sha256};
use utoipa::ToSchema;

use natsume_device_protocol::generated::{
    GatewayActualState as WireGatewayActualState, GatewayState as WireGatewayState,
};

use crate::component::gateway::{GatewayActualState, GatewayCredentialId, MaterializedGateway};

use super::ConvergenceStatusResponse;

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

pub(super) fn gateway_target_response(target: &MaterializedGateway) -> GatewayTargetResponse {
    let target = target.target();
    let gateway_leaf_sha256 = target
        .certificate()
        .map(|grant| hex::encode(Sha256::digest(grant.leaf_der())));
    GatewayTargetResponse {
        credential_id: target.credential_id().as_text(),
        gateway_leaf_sha256,
    }
}

pub(super) fn gateway_actual_response(
    actual: WireGatewayActualState,
) -> Option<(GatewayActualState, GatewayActualResponse)> {
    let state = WireGatewayState::try_from(actual.state).ok()?;
    match (state, actual.credential_id, actual.gateway_leaf_sha256) {
        (WireGatewayState::Absent, None, None) => Some((
            GatewayActualState::Absent,
            GatewayActualResponse {
                credential_id: None,
                state: GatewayStateResponse::Absent,
                gateway_leaf_sha256: None,
            },
        )),
        (WireGatewayState::Restoring, Some(credential_id), None) => {
            let credential_id = GatewayCredentialId::parse(&credential_id)?;
            Some((
                GatewayActualState::Tracking { credential_id },
                GatewayActualResponse {
                    credential_id: Some(credential_id.as_text()),
                    state: GatewayStateResponse::Restoring,
                    gateway_leaf_sha256: None,
                },
            ))
        }
        (WireGatewayState::RecoveryRequired, Some(credential_id), None) => {
            let credential_id = GatewayCredentialId::parse(&credential_id)?;
            Some((
                GatewayActualState::RecoveryRequired { credential_id },
                GatewayActualResponse {
                    credential_id: Some(credential_id.as_text()),
                    state: GatewayStateResponse::RecoveryRequired,
                    gateway_leaf_sha256: None,
                },
            ))
        }
        (
            state @ (WireGatewayState::Blocked
            | WireGatewayState::Ready
            | WireGatewayState::UpstreamUnhealthy),
            Some(credential_id),
            Some(leaf_sha256),
        ) => {
            let credential_id = GatewayCredentialId::parse(&credential_id)?;
            let leaf_sha256: [u8; 32] = leaf_sha256.try_into().ok()?;
            let response_state = match state {
                WireGatewayState::Blocked => GatewayStateResponse::Blocked,
                WireGatewayState::Ready => GatewayStateResponse::Ready,
                WireGatewayState::UpstreamUnhealthy => GatewayStateResponse::UpstreamUnhealthy,
                _ => return None,
            };
            Some((
                GatewayActualState::Loaded {
                    credential_id,
                    leaf_sha256,
                },
                GatewayActualResponse {
                    credential_id: Some(credential_id.as_text()),
                    state: response_state,
                    gateway_leaf_sha256: Some(hex::encode(leaf_sha256)),
                },
            ))
        }
        _ => None,
    }
}

pub(super) fn gateway_convergence_status(
    target: Option<&GatewayTargetResponse>,
    actual: Option<&GatewayActualResponse>,
) -> ConvergenceStatusResponse {
    let (Some(target), Some(actual)) = (target, actual) else {
        return ConvergenceStatusResponse::AwaitingActual;
    };
    if actual.state == GatewayStateResponse::RecoveryRequired {
        return ConvergenceStatusResponse::Failed;
    }
    if target.gateway_leaf_sha256.is_none() {
        return ConvergenceStatusResponse::Reconciling;
    }
    let exact = actual.credential_id.as_deref() == Some(target.credential_id.as_str())
        && actual.gateway_leaf_sha256 == target.gateway_leaf_sha256;
    match (exact, actual.state) {
        (true, GatewayStateResponse::Ready) => ConvergenceStatusResponse::Converged,
        (true, GatewayStateResponse::UpstreamUnhealthy) => ConvergenceStatusResponse::Failed,
        (true, GatewayStateResponse::Blocked | GatewayStateResponse::Restoring) => {
            ConvergenceStatusResponse::Reconciling
        }
        _ => ConvergenceStatusResponse::Drifted,
    }
}
