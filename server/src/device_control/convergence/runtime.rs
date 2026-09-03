use serde::Serialize;
use utoipa::ToSchema;

use natsume_device_protocol::generated::{
    RuntimeConfigActualState, RuntimeConfigState as WireRuntimeConfigState,
};

use crate::component::runtime::is_canonical_https_origin;

use super::ConvergenceStatusResponse;

/// Runtime Config target, Actual, and convergence result.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeConfigConvergenceResponse {
    #[schema(inline)]
    pub(super) status: ConvergenceStatusResponse,
    #[schema(required = true)]
    pub(super) target_domjudge_origin: Option<String>,
    #[schema(required = true)]
    pub(super) actual: Option<RuntimeConfigActualResponse>,
}

/// Latest validated Runtime Config Actual reported by the current lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeConfigActualResponse {
    #[schema(inline)]
    pub(super) state: RuntimeConfigStateResponse,
    #[schema(required = true)]
    pub(super) applied_domjudge_origin: Option<String>,
}

/// Runtime Config Actual state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum RuntimeConfigStateResponse {
    Absent,
    Applied,
    Failed,
}

pub(super) fn runtime_actual_response(
    actual: RuntimeConfigActualState,
) -> Option<RuntimeConfigActualResponse> {
    let state = match WireRuntimeConfigState::try_from(actual.state).ok()? {
        WireRuntimeConfigState::Unspecified => return None,
        WireRuntimeConfigState::Absent => RuntimeConfigStateResponse::Absent,
        WireRuntimeConfigState::Applied => RuntimeConfigStateResponse::Applied,
        WireRuntimeConfigState::Failed => RuntimeConfigStateResponse::Failed,
    };
    match (state, actual.applied_domjudge_origin.as_deref()) {
        (RuntimeConfigStateResponse::Absent | RuntimeConfigStateResponse::Failed, None) => {}
        (
            RuntimeConfigStateResponse::Applied | RuntimeConfigStateResponse::Failed,
            Some(origin),
        ) if is_canonical_https_origin(origin) => {}
        _ => return None,
    }
    Some(RuntimeConfigActualResponse {
        state,
        applied_domjudge_origin: actual.applied_domjudge_origin,
    })
}

pub(super) fn runtime_convergence_status(
    target: Option<&str>,
    actual: Option<&RuntimeConfigActualResponse>,
) -> ConvergenceStatusResponse {
    let (Some(target), Some(actual)) = (target, actual) else {
        return ConvergenceStatusResponse::AwaitingActual;
    };
    match actual.state {
        RuntimeConfigStateResponse::Failed => ConvergenceStatusResponse::Failed,
        RuntimeConfigStateResponse::Applied
            if actual.applied_domjudge_origin.as_deref() == Some(target) =>
        {
            ConvergenceStatusResponse::Converged
        }
        RuntimeConfigStateResponse::Absent | RuntimeConfigStateResponse::Applied => {
            ConvergenceStatusResponse::Drifted
        }
    }
}
