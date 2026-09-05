use natsume_device_protocol::generated::{
    RuntimeConfigActualState, RuntimeConfigState as WireRuntimeConfigState,
};

use crate::component::runtime::is_canonical_https_origin;

use super::ConvergenceStatus;

/// Runtime Config target, Actual, and convergence result.
#[derive(PartialEq, Eq)]
pub(crate) struct RuntimeConfigConvergence {
    pub(crate) status: ConvergenceStatus,
    pub(crate) target_domjudge_origin: Option<String>,
    pub(crate) actual: Option<RuntimeConfigActual>,
}

/// Latest validated Runtime Config Actual reported by the current lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeConfigActual {
    pub(crate) state: RuntimeConfigState,
    pub(crate) applied_domjudge_origin: Option<String>,
}

/// Runtime Config Actual state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeConfigState {
    Absent,
    Applied,
    Failed,
}

pub(super) fn parse_runtime_actual(
    actual: RuntimeConfigActualState,
) -> Option<RuntimeConfigActual> {
    let state = match WireRuntimeConfigState::try_from(actual.state).ok()? {
        WireRuntimeConfigState::Unspecified => return None,
        WireRuntimeConfigState::Absent => RuntimeConfigState::Absent,
        WireRuntimeConfigState::Applied => RuntimeConfigState::Applied,
        WireRuntimeConfigState::Failed => RuntimeConfigState::Failed,
    };
    match (state, actual.applied_domjudge_origin.as_deref()) {
        (RuntimeConfigState::Absent | RuntimeConfigState::Failed, None) => {}
        (RuntimeConfigState::Applied | RuntimeConfigState::Failed, Some(origin))
            if is_canonical_https_origin(origin) => {}
        _ => return None,
    }
    Some(RuntimeConfigActual {
        state,
        applied_domjudge_origin: actual.applied_domjudge_origin,
    })
}

pub(super) fn runtime_convergence_status(
    target: Option<&str>,
    actual: Option<&RuntimeConfigActual>,
) -> ConvergenceStatus {
    let (Some(target), Some(actual)) = (target, actual) else {
        return ConvergenceStatus::AwaitingActual;
    };
    match actual.state {
        RuntimeConfigState::Failed => ConvergenceStatus::Failed,
        RuntimeConfigState::Applied
            if actual.applied_domjudge_origin.as_deref() == Some(target) =>
        {
            ConvergenceStatus::Converged
        }
        RuntimeConfigState::Absent | RuntimeConfigState::Applied => ConvergenceStatus::Drifted,
    }
}
