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
    #[schema(inline)]
    pub(super) foreground: SessionForegroundResponse,
    pub(super) waiting_ready: bool,
    pub(super) contest_ready: bool,
}

impl From<model::SessionActual> for SessionActualResponse {
    fn from(value: model::SessionActual) -> Self {
        Self {
            session_state: value.session_state.into(),
            completed_terminate_epoch: value.completed_terminate_epoch,
            foreground: value.foreground.into(),
            waiting_ready: value.waiting_ready,
            contest_ready: value.contest_ready,
        }
    }
}

/// Session state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum SessionStateResponse {
    None,
    Starting,
    Running,
    Terminating,
    Ambiguous,
    Error,
}

impl From<model::SessionState> for SessionStateResponse {
    fn from(value: model::SessionState) -> Self {
        match value {
            model::SessionState::None => Self::None,
            model::SessionState::Starting => Self::Starting,
            model::SessionState::Running => Self::Running,
            model::SessionState::Terminating => Self::Terminating,
            model::SessionState::Ambiguous => Self::Ambiguous,
            model::SessionState::Error => Self::Error,
        }
    }
}

/// The physical foreground is separate from the requested managed role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum SessionForegroundResponse {
    Unknown,
    Waiting,
    Contest,
    Greeter,
    Other,
    None,
}

impl From<model::SessionForeground> for SessionForegroundResponse {
    fn from(value: model::SessionForeground) -> Self {
        match value {
            model::SessionForeground::Unknown => Self::Unknown,
            model::SessionForeground::Waiting => Self::Waiting,
            model::SessionForeground::Contest => Self::Contest,
            model::SessionForeground::Greeter => Self::Greeter,
            model::SessionForeground::Other => Self::Other,
            model::SessionForeground::None => Self::None,
        }
    }
}
