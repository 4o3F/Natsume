use serde::Serialize;
use utoipa::ToSchema;

use natsume_device_protocol::generated::{HomeActualState, HomeState as WireHomeState};

use super::ConvergenceStatusResponse;

/// Home target, Actual, and convergence result.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeConvergenceResponse {
    #[schema(inline)]
    pub(super) status: ConvergenceStatusResponse,
    #[schema(required = true)]
    pub(super) target_reset_epoch: Option<u64>,
    #[schema(required = true)]
    pub(super) actual: Option<HomeActualResponse>,
}

/// Latest validated Home Actual reported by the current lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeActualResponse {
    #[schema(inline)]
    pub(super) state: HomeStateResponse,
    #[schema(required = true)]
    pub(super) completed_reset_epoch: Option<u64>,
}

/// Home Actual state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum HomeStateResponse {
    Steady,
    Resetting,
    RecoveryRequired,
}

pub(super) fn home_actual_response(actual: HomeActualState) -> Option<HomeActualResponse> {
    let state = match WireHomeState::try_from(actual.state).ok()? {
        WireHomeState::Unspecified => return None,
        WireHomeState::Steady => HomeStateResponse::Steady,
        WireHomeState::Resetting => HomeStateResponse::Resetting,
        WireHomeState::RecoveryRequired => HomeStateResponse::RecoveryRequired,
    };
    if actual
        .completed_reset_epoch
        .is_some_and(|epoch| epoch == 0 || epoch > i64::MAX.cast_unsigned())
    {
        return None;
    }
    Some(HomeActualResponse {
        state,
        completed_reset_epoch: actual.completed_reset_epoch,
    })
}

pub(super) fn home_convergence_status(
    target: Option<u64>,
    actual: Option<&HomeActualResponse>,
) -> ConvergenceStatusResponse {
    let Some(actual) = actual else {
        return ConvergenceStatusResponse::AwaitingActual;
    };
    match actual.state {
        HomeStateResponse::RecoveryRequired => ConvergenceStatusResponse::Failed,
        HomeStateResponse::Resetting => ConvergenceStatusResponse::Reconciling,
        HomeStateResponse::Steady if target == actual.completed_reset_epoch => {
            ConvergenceStatusResponse::Converged
        }
        HomeStateResponse::Steady => ConvergenceStatusResponse::Drifted,
    }
}
