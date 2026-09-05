use serde::Serialize;
use utoipa::ToSchema;

use natsume_device_protocol::generated::ActualState;

use crate::{
    component::{
        binding::{BindingError, BindingProjection},
        device::{DeviceError, DeviceId, DeviceProjection},
        gateway::{GatewayActualState, GatewayError, MaterializedGateway},
        home::HomeError,
        runtime::RuntimeConfigError,
        session::{SessionControlError, SessionControlTarget},
    },
    server_state::ServerState,
};

use super::{DeviceControl, actor::DeviceConnectionState};

mod binding;
mod gateway;
mod home;
mod runtime;
mod session;

use binding::{
    BindingActualResponse, BindingConvergenceResponse, binding_actual_response,
    binding_convergence_status, binding_target_response,
};
use gateway::{
    GatewayActualResponse, GatewayConvergenceResponse, gateway_actual_response,
    gateway_convergence_status, gateway_target_response,
};
use home::{
    HomeActualResponse, HomeConvergenceResponse, home_actual_response, home_convergence_status,
};
use runtime::{
    RuntimeConfigActualResponse, RuntimeConfigConvergenceResponse, runtime_actual_response,
    runtime_convergence_status,
};
use session::{
    SessionActualResponse, SessionConvergenceResponse, session_actual_response,
    session_convergence_status,
};

/// Current durable targets and latest validated Actual for one Device.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceConvergenceResponse {
    #[schema(inline)]
    connection_state: ConnectionStateResponse,
    #[schema(required = true)]
    received_at_unix_ms: Option<i64>,
    gateway: GatewayConvergenceResponse,
    binding: BindingConvergenceResponse,
    runtime_config: RuntimeConfigConvergenceResponse,
    session_control: SessionConvergenceResponse,
    home: HomeConvergenceResponse,
}

/// One durable Device projection paired with its current convergence view.
pub(crate) struct DeviceStatus {
    device: DeviceProjection,
    convergence: DeviceConvergenceResponse,
}

impl DeviceStatus {
    pub(crate) fn into_parts(self) -> (DeviceProjection, DeviceConvergenceResponse) {
        (self.device, self.convergence)
    }
}

#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum ConnectionStateResponse {
    Offline,
    AwaitingFreshState,
    Active,
}

/// Typed comparison of one current target with one fresh Actual.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum ConvergenceStatusResponse {
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
    gateway: GatewayActualResponse,
    binding: BindingActualResponse,
    runtime_config: RuntimeConfigActualResponse,
    session_control: SessionActualResponse,
    home: HomeActualResponse,
}

/// Durable component targets required to calculate one Device convergence view.
struct DeviceTargets {
    gateway: Option<MaterializedGateway>,
    binding: Option<BindingProjection>,
    runtime_config: Option<String>,
    session_control: Option<SessionControlTarget>,
    home: Option<u64>,
}

/// Validates every resource Actual and returns the Gateway state consumed by its component.
pub(super) fn parse_actual(
    actual: ActualState,
) -> Option<(GatewayActualState, ObservedActualState)> {
    let (gateway_component_actual, gateway) = gateway_actual_response(actual.gateway?)?;
    let binding = binding_actual_response(actual.binding_access?)?;
    let runtime_config = runtime_actual_response(actual.runtime_config?)?;
    let session_control = session_actual_response(actual.session_control?)?;
    let home = home_actual_response(actual.home?)?;
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

impl DeviceControl {
    /// Reads current component targets and compares them with the current lease observation.
    pub(crate) async fn read_convergence(
        &self,
        state: &ServerState,
        device_id: DeviceId,
    ) -> Result<Option<DeviceConvergenceResponse>, DeviceConvergenceError> {
        self.read_device_status(state, device_id)
            .await
            .map(|status| status.map(|status| status.convergence))
    }

    /// Reads one durable Device and calculates its current convergence view.
    pub(crate) async fn read_device_status(
        &self,
        state: &ServerState,
        device_id: DeviceId,
    ) -> Result<Option<DeviceStatus>, DeviceConvergenceError> {
        let Some(device) = state
            .device()
            .find_device(device_id)
            .await
            .map_err(DeviceConvergenceError::Device)?
        else {
            return Ok(None);
        };
        let gateway = state
            .gateway()
            .read_current(device_id)
            .await
            .map_err(DeviceConvergenceError::Gateway)?;
        let binding = state
            .binding()
            .read_current(device_id)
            .await
            .map_err(DeviceConvergenceError::Binding)?;
        let runtime_config = state
            .runtime()
            .read_current()
            .await
            .map_err(DeviceConvergenceError::Runtime)?;
        let session_control = state
            .session()
            .read_current(device_id)
            .await
            .map_err(DeviceConvergenceError::Session)?;
        let home = state
            .home()
            .read_current(device_id)
            .await
            .map_err(DeviceConvergenceError::Home)?;
        let targets = DeviceTargets {
            gateway,
            binding,
            runtime_config,
            session_control,
            home,
        };
        let connection = self.registry.read_connection_state(device_id).await;
        Ok(Some(DeviceStatus {
            device,
            convergence: build_convergence(connection, targets),
        }))
    }

    /// Reads every durable Device and calculates all convergence views with fixed-count queries.
    pub(crate) async fn read_all_device_statuses(
        &self,
        state: &ServerState,
    ) -> Result<Vec<DeviceStatus>, DeviceConvergenceError> {
        let devices = state
            .device()
            .list_devices()
            .await
            .map_err(DeviceConvergenceError::Device)?;
        let device_ids = devices
            .iter()
            .map(|device| device.device_id())
            .collect::<Vec<_>>();
        let mut gateways = state
            .gateway()
            .read_all_current()
            .await
            .map_err(DeviceConvergenceError::Gateway)?;
        let mut bindings = state
            .binding()
            .read_all_current()
            .await
            .map_err(DeviceConvergenceError::Binding)?;
        let runtime_config = state
            .runtime()
            .read_current()
            .await
            .map_err(DeviceConvergenceError::Runtime)?;
        let mut sessions = state
            .session()
            .read_all_current()
            .await
            .map_err(DeviceConvergenceError::Session)?;
        let mut homes = state
            .home()
            .read_all_current()
            .await
            .map_err(DeviceConvergenceError::Home)?;
        let mut connections = self.registry.read_connection_states(&device_ids).await;

        Ok(devices
            .into_iter()
            .map(|device| {
                let device_id = device.device_id();
                let targets = DeviceTargets {
                    gateway: gateways.remove(&device_id),
                    binding: bindings.remove(&device_id),
                    runtime_config: runtime_config.clone(),
                    session_control: sessions.remove(&device_id),
                    home: homes.remove(&device_id).flatten(),
                };
                let connection = connections
                    .remove(&device_id)
                    .unwrap_or(DeviceConnectionState::Offline);
                DeviceStatus {
                    device,
                    convergence: build_convergence(connection, targets),
                }
            })
            .collect())
    }
}

fn build_convergence(
    connection: DeviceConnectionState,
    targets: DeviceTargets,
) -> DeviceConvergenceResponse {
    let DeviceTargets {
        gateway,
        binding,
        runtime_config,
        session_control,
        home,
    } = targets;
    let gateway = gateway.as_ref().map(gateway_target_response);
    let binding = binding.map(binding_target_response);
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

    DeviceConvergenceResponse {
        connection_state,
        received_at_unix_ms,
        gateway: GatewayConvergenceResponse {
            status: gateway_convergence_status(gateway.as_ref(), gateway_actual.as_ref()),
            target: gateway,
            actual: gateway_actual,
        },
        binding: BindingConvergenceResponse {
            status: binding_convergence_status(binding.as_ref(), binding_actual.as_ref()),
            target: binding,
            actual: binding_actual,
        },
        runtime_config: RuntimeConfigConvergenceResponse {
            status: runtime_convergence_status(runtime_config.as_deref(), runtime_actual.as_ref()),
            target_domjudge_origin: runtime_config,
            actual: runtime_actual,
        },
        session_control: SessionConvergenceResponse {
            status: session_status,
            target: session_control.map(Into::into),
            actual: session_actual,
        },
        home: HomeConvergenceResponse {
            status: home_convergence_status(home, home_actual.as_ref()),
            target_reset_epoch: home,
            actual: home_actual,
        },
    }
}

fn connection_observation(
    state: DeviceConnectionState,
) -> (
    ConnectionStateResponse,
    Option<i64>,
    Option<ObservedActualState>,
) {
    match state {
        DeviceConnectionState::Offline => (ConnectionStateResponse::Offline, None, None),
        DeviceConnectionState::AwaitingFreshState => {
            (ConnectionStateResponse::AwaitingFreshState, None, None)
        }
        DeviceConnectionState::Active {
            actual,
            received_at_unix_ms,
        } => (
            ConnectionStateResponse::Active,
            Some(received_at_unix_ms),
            Some(*actual),
        ),
    }
}

#[cfg(test)]
mod tests {
    use crate::component::session::LockState;

    use super::{
        BindingActualResponse, ConvergenceStatusResponse, GatewayActualResponse,
        RuntimeConfigActualResponse, SessionActualResponse,
        binding::{BindingArtifactStateResponse, BindingTargetResponse},
        binding_convergence_status,
        gateway::{GatewayStateResponse, GatewayTargetResponse},
        gateway_convergence_status,
        home::{HomeActualResponse, HomeStateResponse},
        home_convergence_status,
        runtime::RuntimeConfigStateResponse,
        runtime_convergence_status,
        session::SessionStateResponse,
        session_convergence_status,
    };

    #[test]
    fn missing_fresh_actual_is_never_reported_as_converged() {
        let gateway = GatewayTargetResponse {
            credential_id: "01900000-0000-7000-8000-000000000001".to_owned(),
            gateway_leaf_sha256: Some("01".repeat(32)),
        };
        let binding = BindingTargetResponse::Unbound {
            negotiation_id: "01900000-0000-7000-8000-000000000002".to_owned(),
            evaluation: None,
        };

        assert_eq!(
            gateway_convergence_status(Some(&gateway), None),
            ConvergenceStatusResponse::AwaitingActual
        );
        assert_eq!(
            binding_convergence_status(Some(&binding), None),
            ConvergenceStatusResponse::AwaitingActual
        );
        assert_eq!(
            runtime_convergence_status(Some("https://example.test"), None),
            ConvergenceStatusResponse::AwaitingActual
        );
        assert_eq!(
            session_convergence_status(Some((LockState::Unlocked, None)), None),
            ConvergenceStatusResponse::AwaitingActual
        );
        assert_eq!(
            home_convergence_status(None, None),
            ConvergenceStatusResponse::AwaitingActual
        );
    }

    #[test]
    fn exact_resource_actuals_are_reported_as_converged() {
        let leaf_hash = "01".repeat(32);
        let gateway = GatewayTargetResponse {
            credential_id: "01900000-0000-7000-8000-000000000001".to_owned(),
            gateway_leaf_sha256: Some(leaf_hash.clone()),
        };
        let gateway_actual = GatewayActualResponse {
            credential_id: Some(gateway.credential_id.clone()),
            state: GatewayStateResponse::Ready,
            gateway_leaf_sha256: Some(leaf_hash),
        };
        let binding = BindingTargetResponse::Unbound {
            negotiation_id: "01900000-0000-7000-8000-000000000002".to_owned(),
            evaluation: None,
        };
        let binding_actual = BindingActualResponse {
            assignment_state: BindingArtifactStateResponse::Absent,
            credential_state: BindingArtifactStateResponse::Absent,
            context: None,
        };
        let runtime_actual = RuntimeConfigActualResponse {
            state: RuntimeConfigStateResponse::Applied,
            applied_domjudge_origin: Some("https://example.test".to_owned()),
        };
        let session_actual = SessionActualResponse {
            session_state: SessionStateResponse::Active,
            completed_terminate_epoch: None,
        };
        let home_actual = HomeActualResponse {
            state: HomeStateResponse::Steady,
            completed_reset_epoch: None,
        };

        assert_eq!(
            gateway_convergence_status(Some(&gateway), Some(&gateway_actual)),
            ConvergenceStatusResponse::Converged
        );
        assert_eq!(
            binding_convergence_status(Some(&binding), Some(&binding_actual)),
            ConvergenceStatusResponse::Converged
        );
        assert_eq!(
            runtime_convergence_status(Some("https://example.test"), Some(&runtime_actual)),
            ConvergenceStatusResponse::Converged
        );
        assert_eq!(
            session_convergence_status(Some((LockState::Unlocked, None)), Some(&session_actual)),
            ConvergenceStatusResponse::Converged
        );
        assert_eq!(
            home_convergence_status(None, Some(&home_actual)),
            ConvergenceStatusResponse::Converged
        );
    }
}
