use natsume_device_protocol::generated::{
    SessionControlActualState, SessionState as WireSessionState,
};

use crate::component::session::{LockState, SessionControlTarget};

use super::ConvergenceStatus;

/// Session Control target, Actual, and convergence result.
#[derive(PartialEq, Eq)]
pub(crate) struct SessionConvergence {
    pub(crate) status: ConvergenceStatus,
    pub(crate) target: Option<SessionControlTarget>,
    pub(crate) actual: Option<SessionActual>,
}

/// Latest validated Session Control Actual reported by the current lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionActual {
    pub(crate) session_state: SessionState,
    pub(crate) completed_terminate_epoch: Option<u64>,
}

/// Session state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionState {
    None,
    Starting,
    Active,
    Locked,
    Terminating,
    Ambiguous,
    Error,
}

pub(super) fn parse_session_actual(actual: SessionControlActualState) -> Option<SessionActual> {
    let session_state = match WireSessionState::try_from(actual.session_state).ok()? {
        WireSessionState::Unspecified => return None,
        WireSessionState::None => SessionState::None,
        WireSessionState::Starting => SessionState::Starting,
        WireSessionState::Active => SessionState::Active,
        WireSessionState::Locked => SessionState::Locked,
        WireSessionState::Terminating => SessionState::Terminating,
        WireSessionState::Ambiguous => SessionState::Ambiguous,
        WireSessionState::Error => SessionState::Error,
    };
    if actual
        .completed_terminate_epoch
        .is_some_and(|epoch| epoch == 0 || epoch > i64::MAX.cast_unsigned())
    {
        return None;
    }
    Some(SessionActual {
        session_state,
        completed_terminate_epoch: actual.completed_terminate_epoch,
    })
}

pub(super) fn session_convergence_status(
    target: Option<(LockState, Option<u64>)>,
    actual: Option<&SessionActual>,
) -> ConvergenceStatus {
    let (Some((lock_state, terminate_epoch)), Some(actual)) = (target, actual) else {
        return ConvergenceStatus::AwaitingActual;
    };
    if matches!(
        actual.session_state,
        SessionState::Ambiguous | SessionState::Error
    ) {
        return ConvergenceStatus::Failed;
    }
    if terminate_epoch > actual.completed_terminate_epoch
        || actual.session_state == SessionState::Terminating
    {
        return ConvergenceStatus::Reconciling;
    }
    let lock_converged = match lock_state {
        LockState::Locked => actual.session_state == SessionState::Locked,
        LockState::Unlocked => actual.session_state != SessionState::Locked,
    };
    if lock_converged && terminate_epoch == actual.completed_terminate_epoch {
        ConvergenceStatus::Converged
    } else {
        ConvergenceStatus::Drifted
    }
}
