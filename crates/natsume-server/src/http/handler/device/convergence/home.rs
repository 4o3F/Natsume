use serde::Serialize;

use super::ConvergenceStatusResponse;
use crate::device_control as model;

/// Home target, Actual, and convergence result.
#[derive(PartialEq, Eq, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeConvergenceResponse {
    pub(super) status: ConvergenceStatusResponse,
    #[specta(type = Option<specta_typescript::Number>)]
    pub(super) target_reset_epoch: Option<u64>,
    pub(super) actual: Option<HomeActualResponse>,
}

impl From<model::HomeConvergence> for HomeConvergenceResponse {
    fn from(value: model::HomeConvergence) -> Self {
        Self {
            status: value.status.into(),
            target_reset_epoch: value.target_reset_epoch,
            actual: value.actual.map(Into::into),
        }
    }
}

/// Latest validated Home Actual reported by the current lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeActualResponse {
    pub(super) state: HomeStateResponse,
    #[specta(type = Option<specta_typescript::Number>)]
    pub(super) completed_reset_epoch: Option<u64>,
}

impl From<model::HomeActual> for HomeActualResponse {
    fn from(value: model::HomeActual) -> Self {
        Self {
            state: value.state.into(),
            completed_reset_epoch: value.completed_reset_epoch,
        }
    }
}

/// Home Actual state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(super) enum HomeStateResponse {
    Steady,
    Resetting,
    RecoveryRequired,
}

impl From<model::HomeState> for HomeStateResponse {
    fn from(value: model::HomeState) -> Self {
        match value {
            model::HomeState::Steady => Self::Steady,
            model::HomeState::Resetting => Self::Resetting,
            model::HomeState::RecoveryRequired => Self::RecoveryRequired,
        }
    }
}
