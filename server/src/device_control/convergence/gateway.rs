use sha2::{Digest as _, Sha256};

use natsume_device_protocol::generated::{
    GatewayActualState as WireGatewayActualState, GatewayState as WireGatewayState,
};

use crate::component::gateway::{GatewayActualState, GatewayCredentialId, MaterializedGateway};

use super::ConvergenceStatus;

/// Redacted Gateway target, Actual, and convergence result.
#[derive(PartialEq, Eq)]
pub(crate) struct GatewayConvergence {
    pub(crate) status: ConvergenceStatus,
    pub(crate) target: Option<GatewayTarget>,
    pub(crate) actual: Option<GatewayActual>,
}

/// Public fields required to compare the current Gateway target.
#[derive(PartialEq, Eq)]
pub(crate) struct GatewayTarget {
    pub(crate) credential_id: String,
    pub(crate) gateway_leaf_sha256: Option<String>,
}

/// Latest validated Gateway Actual reported by the current lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayActual {
    pub(crate) credential_id: Option<String>,
    pub(crate) state: GatewayState,
    pub(crate) gateway_leaf_sha256: Option<String>,
}

/// Gateway Actual state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GatewayState {
    Absent,
    Blocked,
    Restoring,
    Ready,
    UpstreamUnhealthy,
    RecoveryRequired,
}

pub(super) fn gateway_target(target: &MaterializedGateway) -> GatewayTarget {
    let target = target.target();
    let gateway_leaf_sha256 = target
        .certificate()
        .map(|grant| hex::encode(Sha256::digest(grant.leaf_der())));
    GatewayTarget {
        credential_id: target.credential_id().as_text(),
        gateway_leaf_sha256,
    }
}

pub(super) fn parse_gateway_actual(
    actual: WireGatewayActualState,
) -> Option<(GatewayActualState, GatewayActual)> {
    let state = WireGatewayState::try_from(actual.state).ok()?;
    match (state, actual.credential_id, actual.gateway_leaf_sha256) {
        (WireGatewayState::Absent, None, None) => Some((
            GatewayActualState::Absent,
            GatewayActual {
                credential_id: None,
                state: GatewayState::Absent,
                gateway_leaf_sha256: None,
            },
        )),
        (WireGatewayState::Restoring, Some(credential_id), None) => {
            let credential_id = GatewayCredentialId::parse(&credential_id)?;
            Some((
                GatewayActualState::Tracking { credential_id },
                GatewayActual {
                    credential_id: Some(credential_id.as_text()),
                    state: GatewayState::Restoring,
                    gateway_leaf_sha256: None,
                },
            ))
        }
        (WireGatewayState::RecoveryRequired, Some(credential_id), None) => {
            let credential_id = GatewayCredentialId::parse(&credential_id)?;
            Some((
                GatewayActualState::RecoveryRequired { credential_id },
                GatewayActual {
                    credential_id: Some(credential_id.as_text()),
                    state: GatewayState::RecoveryRequired,
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
            let observed_state = match state {
                WireGatewayState::Blocked => GatewayState::Blocked,
                WireGatewayState::Ready => GatewayState::Ready,
                WireGatewayState::UpstreamUnhealthy => GatewayState::UpstreamUnhealthy,
                _ => return None,
            };
            Some((
                GatewayActualState::Loaded {
                    credential_id,
                    leaf_sha256,
                },
                GatewayActual {
                    credential_id: Some(credential_id.as_text()),
                    state: observed_state,
                    gateway_leaf_sha256: Some(hex::encode(leaf_sha256)),
                },
            ))
        }
        _ => None,
    }
}

pub(super) fn gateway_convergence_status(
    target: Option<&GatewayTarget>,
    actual: Option<&GatewayActual>,
) -> ConvergenceStatus {
    let (Some(target), Some(actual)) = (target, actual) else {
        return ConvergenceStatus::AwaitingActual;
    };
    if actual.state == GatewayState::RecoveryRequired {
        return ConvergenceStatus::Failed;
    }
    if target.gateway_leaf_sha256.is_none() {
        return ConvergenceStatus::Reconciling;
    }
    let exact = actual.credential_id.as_deref() == Some(target.credential_id.as_str())
        && actual.gateway_leaf_sha256 == target.gateway_leaf_sha256;
    match (exact, actual.state) {
        (true, GatewayState::Ready) => ConvergenceStatus::Converged,
        (true, GatewayState::UpstreamUnhealthy) => ConvergenceStatus::Failed,
        (true, GatewayState::Blocked | GatewayState::Restoring) => ConvergenceStatus::Reconciling,
        _ => ConvergenceStatus::Drifted,
    }
}
