use serde::Serialize;
use utoipa::ToSchema;

use natsume_device_protocol::generated::{
    SessionControlActualState, SessionState as WireSessionState,
};

use crate::{
    component::session::LockState, http::handler::device::session::SessionControlTargetResponse,
};

use super::ConvergenceStatusResponse;

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

/// Latest validated Session Control Actual reported by the current lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionActualResponse {
    #[schema(inline)]
    pub(super) session_state: SessionStateResponse,
    #[schema(required = true)]
    pub(super) completed_terminate_epoch: Option<u64>,
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

pub(super) fn session_actual_response(
    actual: SessionControlActualState,
) -> Option<SessionActualResponse> {
    let session_state = match WireSessionState::try_from(actual.session_state).ok()? {
        WireSessionState::Unspecified => return None,
        WireSessionState::None => SessionStateResponse::None,
        WireSessionState::Starting => SessionStateResponse::Starting,
        WireSessionState::Active => SessionStateResponse::Active,
        WireSessionState::Locked => SessionStateResponse::Locked,
        WireSessionState::Terminating => SessionStateResponse::Terminating,
        WireSessionState::Ambiguous => SessionStateResponse::Ambiguous,
        WireSessionState::Error => SessionStateResponse::Error,
    };
    if actual
        .completed_terminate_epoch
        .is_some_and(|epoch| epoch == 0 || epoch > i64::MAX.cast_unsigned())
    {
        return None;
    }
    Some(SessionActualResponse {
        session_state,
        completed_terminate_epoch: actual.completed_terminate_epoch,
    })
}

pub(super) fn session_convergence_status(
    target: Option<(LockState, Option<u64>)>,
    actual: Option<&SessionActualResponse>,
) -> ConvergenceStatusResponse {
    let (Some((lock_state, terminate_epoch)), Some(actual)) = (target, actual) else {
        return ConvergenceStatusResponse::AwaitingActual;
    };
    if matches!(
        actual.session_state,
        SessionStateResponse::Ambiguous | SessionStateResponse::Error
    ) {
        return ConvergenceStatusResponse::Failed;
    }
    if terminate_epoch > actual.completed_terminate_epoch
        || actual.session_state == SessionStateResponse::Terminating
    {
        return ConvergenceStatusResponse::Reconciling;
    }
    let lock_converged = match lock_state {
        LockState::Locked => actual.session_state == SessionStateResponse::Locked,
        LockState::Unlocked => actual.session_state != SessionStateResponse::Locked,
    };
    if lock_converged && terminate_epoch == actual.completed_terminate_epoch {
        ConvergenceStatusResponse::Converged
    } else {
        ConvergenceStatusResponse::Drifted
    }
}
