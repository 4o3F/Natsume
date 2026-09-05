use natsume_device_protocol::generated::ActualState;

use crate::component::{
    binding::{BindingError, BindingProjection},
    device::{DeviceError, DeviceProjection},
    gateway::{GatewayActualState, GatewayError, MaterializedGateway},
    home::HomeError,
    runtime::RuntimeConfigError,
    session::{SessionControlError, SessionControlTarget},
};

use super::actor::DeviceConnectionState;

mod binding;
mod gateway;
mod home;
mod runtime;
mod session;

pub(crate) use binding::{
    BindingActual, BindingArtifactState, BindingContext, BindingConvergence, BindingEvaluation,
    BindingEvaluationCode, BindingTarget,
};
pub(crate) use gateway::{GatewayActual, GatewayConvergence, GatewayState, GatewayTarget};
pub(crate) use home::{HomeActual, HomeConvergence, HomeState};
pub(crate) use runtime::{RuntimeConfigActual, RuntimeConfigConvergence, RuntimeConfigState};
pub(crate) use session::{SessionActual, SessionConvergence, SessionState};

use binding::{binding_convergence_status, binding_target, parse_binding_actual};
use gateway::{gateway_convergence_status, gateway_target, parse_gateway_actual};
use home::{home_convergence_status, parse_home_actual};
use runtime::{parse_runtime_actual, runtime_convergence_status};
use session::{parse_session_actual, session_convergence_status};

/// Current durable targets and latest validated Actual for one Device.
#[derive(PartialEq, Eq)]
pub(crate) struct DeviceConvergence {
    pub(crate) connection_state: ConnectionState,
    pub(crate) received_at_unix_ms: Option<i64>,
    pub(crate) gateway: GatewayConvergence,
    pub(crate) binding: BindingConvergence,
    pub(crate) runtime_config: RuntimeConfigConvergence,
    pub(crate) session_control: SessionConvergence,
    pub(crate) home: HomeConvergence,
}

/// One durable Device projection paired with its current convergence view.
pub(crate) struct DeviceStatus {
    pub(crate) device: DeviceProjection,
    pub(crate) convergence: DeviceConvergence,
}

#[derive(PartialEq, Eq)]
pub(crate) enum ConnectionState {
    Offline,
    AwaitingFreshState,
    Active,
}

/// Typed comparison of one current target with one fresh Actual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConvergenceStatus {
    AwaitingActual,
    Converged,
    Reconciling,
    Drifted,
    Failed,
}

/// Component failure encountered while assembling one convergence projection.
#[derive(Clone, Copy)]
pub(crate) enum DeviceConvergenceError {
    Device(DeviceError),
    Gateway(GatewayError),
    Binding(BindingError),
    Runtime(RuntimeConfigError),
    Session(SessionControlError),
    Home(HomeError),
}

/// Complete validated Actual retained only for the current Device lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ObservedActualState {
    gateway: GatewayActual,
    binding: BindingActual,
    runtime_config: RuntimeConfigActual,
    session_control: SessionActual,
    home: HomeActual,
}

/// Durable component targets required to calculate one Device convergence view.
pub(super) struct DeviceTargets {
    pub(crate) gateway: Option<MaterializedGateway>,
    pub(crate) binding: Option<BindingProjection>,
    pub(crate) runtime_config: Option<String>,
    pub(crate) session_control: Option<SessionControlTarget>,
    pub(crate) home: Option<u64>,
}

/// Validates every resource Actual and returns the Gateway state consumed by its component.
pub(super) fn parse_actual(
    actual: ActualState,
) -> Option<(GatewayActualState, ObservedActualState)> {
    let (gateway_component_actual, gateway) = parse_gateway_actual(actual.gateway?)?;
    let binding = parse_binding_actual(actual.binding_access?)?;
    let runtime_config = parse_runtime_actual(actual.runtime_config?)?;
    let session_control = parse_session_actual(actual.session_control?)?;
    let home = parse_home_actual(actual.home?)?;
    Some((
        gateway_component_actual,
        ObservedActualState {
            gateway,
            binding,
            runtime_config,
            session_control,
            home,
        },
    ))
}

pub(super) fn build_convergence(
    connection: DeviceConnectionState,
    targets: DeviceTargets,
) -> DeviceConvergence {
    let DeviceTargets {
        gateway,
        binding,
        runtime_config,
        session_control,
        home,
    } = targets;
    let gateway = gateway.as_ref().map(gateway_target);
    let binding = binding.map(binding_target);
    let (connection_state, received_at_unix_ms, actual) = connection_observation(connection);
    let (gateway_actual, binding_actual, runtime_actual, session_actual, home_actual) = match actual
    {
        Some(actual) => (
            Some(actual.gateway),
            Some(actual.binding),
            Some(actual.runtime_config),
            Some(actual.session_control),
            Some(actual.home),
        ),
        None => (None, None, None, None, None),
    };

    let session_status = session_convergence_status(
        session_control
            .as_ref()
            .map(|target| (target.lock_state(), target.terminate_epoch())),
        session_actual.as_ref(),
    );

    DeviceConvergence {
        connection_state,
        received_at_unix_ms,
        gateway: GatewayConvergence {
            status: gateway_convergence_status(gateway.as_ref(), gateway_actual.as_ref()),
            target: gateway,
            actual: gateway_actual,
        },
        binding: BindingConvergence {
            status: binding_convergence_status(binding.as_ref(), binding_actual.as_ref()),
            target: binding,
            actual: binding_actual,
        },
        runtime_config: RuntimeConfigConvergence {
            status: runtime_convergence_status(runtime_config.as_deref(), runtime_actual.as_ref()),
            target_domjudge_origin: runtime_config,
            actual: runtime_actual,
        },
        session_control: SessionConvergence {
            status: session_status,
            target: session_control,
            actual: session_actual,
        },
        home: HomeConvergence {
            status: home_convergence_status(home, home_actual.as_ref()),
            target_reset_epoch: home,
            actual: home_actual,
        },
    }
}

fn connection_observation(
    state: DeviceConnectionState,
) -> (ConnectionState, Option<i64>, Option<ObservedActualState>) {
    match state {
        DeviceConnectionState::Offline => (ConnectionState::Offline, None, None),
        DeviceConnectionState::AwaitingFreshState => {
            (ConnectionState::AwaitingFreshState, None, None)
        }
        DeviceConnectionState::Active {
            actual,
            received_at_unix_ms,
        } => (
            ConnectionState::Active,
            Some(received_at_unix_ms),
            Some(*actual),
        ),
    }
}

#[cfg(test)]
mod tests {
    use crate::component::session::LockState;

    use super::{
        BindingActual, ConvergenceStatus, GatewayActual, RuntimeConfigActual, SessionActual,
        binding::{BindingArtifactState, BindingTarget},
        binding_convergence_status,
        gateway::{GatewayState, GatewayTarget},
        gateway_convergence_status,
        home::{HomeActual, HomeState},
        home_convergence_status,
        runtime::RuntimeConfigState,
        runtime_convergence_status,
        session::SessionState,
        session_convergence_status,
    };

    #[test]
    fn missing_fresh_actual_is_never_reported_as_converged() {
        let gateway = GatewayTarget {
            credential_id: "01900000-0000-7000-8000-000000000001".to_owned(),
            gateway_leaf_sha256: Some("01".repeat(32)),
        };
        let binding = BindingTarget::Unbound {
            negotiation_id: "01900000-0000-7000-8000-000000000002".to_owned(),
            evaluation: None,
        };

        assert_eq!(
            gateway_convergence_status(Some(&gateway), None),
            ConvergenceStatus::AwaitingActual
        );
        assert_eq!(
            binding_convergence_status(Some(&binding), None),
            ConvergenceStatus::AwaitingActual
        );
        assert_eq!(
            runtime_convergence_status(Some("https://example.test"), None),
            ConvergenceStatus::AwaitingActual
        );
        assert_eq!(
            session_convergence_status(Some((LockState::Unlocked, None)), None),
            ConvergenceStatus::AwaitingActual
        );
        assert_eq!(
            home_convergence_status(None, None),
            ConvergenceStatus::AwaitingActual
        );
    }

    #[test]
    fn exact_resource_actuals_are_reported_as_converged() {
        let leaf_hash = "01".repeat(32);
        let gateway = GatewayTarget {
            credential_id: "01900000-0000-7000-8000-000000000001".to_owned(),
            gateway_leaf_sha256: Some(leaf_hash.clone()),
        };
        let gateway_actual = GatewayActual {
            credential_id: Some(gateway.credential_id.clone()),
            state: GatewayState::Ready,
            gateway_leaf_sha256: Some(leaf_hash),
        };
        let binding = BindingTarget::Unbound {
            negotiation_id: "01900000-0000-7000-8000-000000000002".to_owned(),
            evaluation: None,
        };
        let binding_actual = BindingActual {
            assignment_state: BindingArtifactState::Absent,
            credential_state: BindingArtifactState::Absent,
            context: None,
        };
        let runtime_actual = RuntimeConfigActual {
            state: RuntimeConfigState::Applied,
            applied_domjudge_origin: Some("https://example.test".to_owned()),
        };
        let session_actual = SessionActual {
            session_state: SessionState::Active,
            completed_terminate_epoch: None,
        };
        let home_actual = HomeActual {
            state: HomeState::Steady,
            completed_reset_epoch: None,
        };

        assert_eq!(
            gateway_convergence_status(Some(&gateway), Some(&gateway_actual)),
            ConvergenceStatus::Converged
        );
        assert_eq!(
            binding_convergence_status(Some(&binding), Some(&binding_actual)),
            ConvergenceStatus::Converged
        );
        assert_eq!(
            runtime_convergence_status(Some("https://example.test"), Some(&runtime_actual)),
            ConvergenceStatus::Converged
        );
        assert_eq!(
            session_convergence_status(Some((LockState::Unlocked, None)), Some(&session_actual)),
            ConvergenceStatus::Converged
        );
        assert_eq!(
            home_convergence_status(None, Some(&home_actual)),
            ConvergenceStatus::Converged
        );
    }
}
