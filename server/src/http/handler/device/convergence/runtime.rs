use serde::Serialize;
use utoipa::ToSchema;

use super::ConvergenceStatusResponse;
use crate::device_control as model;

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

impl From<model::RuntimeConfigConvergence> for RuntimeConfigConvergenceResponse {
    fn from(value: model::RuntimeConfigConvergence) -> Self {
        Self {
            status: value.status.into(),
            target_domjudge_origin: value.target_domjudge_origin,
            actual: value.actual.map(Into::into),
        }
    }
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

impl From<model::RuntimeConfigActual> for RuntimeConfigActualResponse {
    fn from(value: model::RuntimeConfigActual) -> Self {
        Self {
            state: value.state.into(),
            applied_domjudge_origin: value.applied_domjudge_origin,
        }
    }
}

/// Runtime Config Actual state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum RuntimeConfigStateResponse {
    Absent,
    Applied,
    Failed,
}

impl From<model::RuntimeConfigState> for RuntimeConfigStateResponse {
    fn from(value: model::RuntimeConfigState) -> Self {
        match value {
            model::RuntimeConfigState::Absent => Self::Absent,
            model::RuntimeConfigState::Applied => Self::Applied,
            model::RuntimeConfigState::Failed => Self::Failed,
        }
    }
}
