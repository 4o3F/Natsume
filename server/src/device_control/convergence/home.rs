use natsume_device_protocol::generated::{HomeActualState, HomeState as WireHomeState};

use super::ConvergenceStatus;

/// Home target, Actual, and convergence result.
#[derive(PartialEq, Eq)]
pub(crate) struct HomeConvergence {
    pub(crate) status: ConvergenceStatus,
    pub(crate) target_reset_epoch: Option<u64>,
    pub(crate) actual: Option<HomeActual>,
}

/// Latest validated Home Actual reported by the current lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HomeActual {
    pub(crate) state: HomeState,
    pub(crate) completed_reset_epoch: Option<u64>,
}

/// Home Actual state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HomeState {
    Steady,
    Resetting,
    RecoveryRequired,
}

pub(super) fn parse_home_actual(actual: HomeActualState) -> Option<HomeActual> {
    let state = match WireHomeState::try_from(actual.state).ok()? {
        WireHomeState::Unspecified => return None,
        WireHomeState::Steady => HomeState::Steady,
        WireHomeState::Resetting => HomeState::Resetting,
        WireHomeState::RecoveryRequired => HomeState::RecoveryRequired,
    };
    if actual
        .completed_reset_epoch
        .is_some_and(|epoch| epoch == 0 || epoch > i64::MAX.cast_unsigned())
    {
        return None;
    }
    Some(HomeActual {
        state,
        completed_reset_epoch: actual.completed_reset_epoch,
    })
}

pub(super) fn home_convergence_status(
    target: Option<u64>,
    actual: Option<&HomeActual>,
) -> ConvergenceStatus {
    let Some(actual) = actual else {
        return ConvergenceStatus::AwaitingActual;
    };
    match actual.state {
        HomeState::RecoveryRequired => ConvergenceStatus::Failed,
        HomeState::Resetting => ConvergenceStatus::Reconciling,
        HomeState::Steady if target == actual.completed_reset_epoch => ConvergenceStatus::Converged,
        HomeState::Steady => ConvergenceStatus::Drifted,
    }
}
