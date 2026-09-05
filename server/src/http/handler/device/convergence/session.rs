use serde::Serialize;
use utoipa::ToSchema;

use super::super::session::SessionControlTargetResponse;
use super::ConvergenceStatusResponse;
use crate::device_control as model;

/// Session Control target, Actual, and convergence result.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionConvergenceResponse {
    #[schema(inline)]
    pub(super) status: ConvergenceStatusResponse,
    #[schema(required = true)]
    pub(super) target: Option<SessionControlTargetResponse>,
    #[schema(required = true)]
    pub(super) actual: Option<SessionActualResponse>,
}

impl From<model::SessionConvergence> for SessionConvergenceResponse {
    fn from(value: model::SessionConvergence) -> Self {
        Self {
            status: value.status.into(),
            target: value.target.map(Into::into),
            actual: value.actual.map(Into::into),
        }
    }
}

/// Latest validated Session Control Actual reported by the current lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionActualResponse {
    #[schema(inline)]
    pub(super) session_state: SessionStateResponse,
    #[schema(required = true)]
    pub(super) completed_terminate_epoch: Option<u64>,
}

impl From<model::SessionActual> for SessionActualResponse {
    fn from(value: model::SessionActual) -> Self {
        Self {
            session_state: value.session_state.into(),
            completed_terminate_epoch: value.completed_terminate_epoch,
        }
    }
}

/// Session state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum SessionStateResponse {
    None,
    Starting,
    Active,
    Locked,
    Terminating,
    Ambiguous,
    Error,
}

impl From<model::SessionState> for SessionStateResponse {
    fn from(value: model::SessionState) -> Self {
        match value {
            model::SessionState::None => Self::None,
            model::SessionState::Starting => Self::Starting,
            model::SessionState::Active => Self::Active,
            model::SessionState::Locked => Self::Locked,
            model::SessionState::Terminating => Self::Terminating,
            model::SessionState::Ambiguous => Self::Ambiguous,
            model::SessionState::Error => Self::Error,
        }
    }
}
