use std::{path::Path, sync::Arc};

use natsume_device_protocol::generated::{
    ActualState, ClientInputState, ClientStateSnapshot, PowerControlActualState,
    ServerStateSnapshot,
};
use natsume_local_control_api::ResourceControlError;
use snafu::Snafu;
use tokio_util::sync::CancellationToken;

use crate::canonical_uuid_v7;

mod access;
mod binding;
mod caddy;
mod configuration;
mod gateway;
mod home;
mod maintenance;
mod power;
mod presentation;
mod runtime;
mod session;

use access::{AccessOutcome, LocalAccessObservation, local_access_is_allowed};
use binding::{
    BindingInputProvider, BindingReconciler, DeviceService, ValidatedBindingIntent,
    ValidatedBindingTarget,
};
use caddy::Caddy;
use configuration::AppliedConfiguration;
use gateway::{GatewayReconciler, ValidatedGatewayTarget};
use home::HomeReconciler;
use power::{PowerReconciler, ValidatedPowerTarget};
use runtime::{RuntimeReconciler, is_canonical_https_origin};
use session::{SessionReconciler, ValidatedSessionTarget};

/// Redacted failure at the Client snapshot boundary.
#[derive(Debug, Snafu)]
pub(crate) enum SnapshotError {
    #[snafu(display("Server state snapshot is invalid"))]
    InvalidServerSnapshot,

    #[snafu(display("waiting presentation cache could not be read or persisted"))]
    PresentationCache,

    #[snafu(display("Client resource artifact is unavailable"))]
    Artifact,

    #[snafu(display("Caddy state could not be applied or sampled"))]
    Caddy,

    #[snafu(display("local capability service is unavailable"))]
    LocalControl,

    #[snafu(display("a newer Server state snapshot replaced this plan"))]
    Cancelled,

    #[snafu(display("stale local Binding input was rejected"))]
    StaleLocalInput,
}

/// Resource observation and the independently owned decision to retry local work.
pub(crate) struct ReconcileOutcome<T> {
    pub(crate) actual: T,
    pub(crate) retry: bool,
}

impl<T> ReconcileOutcome<T> {
    fn idle(actual: T) -> Self {
        Self {
            actual,
            retry: false,
        }
    }

    fn retry(actual: T) -> Self {
        Self {
            actual,
            retry: true,
        }
    }

    fn control_error(actual: T, error: &ResourceControlError) -> Self {
        tracing::warn!(%error, "Local resource operation did not complete");
        Self {
            actual,
            retry: retryable_control_error(error),
        }
    }
}

fn retryable_control_error(error: &ResourceControlError) -> bool {
    match error {
        ResourceControlError::Unavailable(_) => true,
        ResourceControlError::Rejected(_) => false,
        ResourceControlError::ZBus(error) => match error {
            zbus::Error::InputOutput(_) => true,
            zbus::Error::MethodError(name, _, _) => matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.NoReply"
                    | "org.freedesktop.DBus.Error.Timeout"
                    | "org.freedesktop.DBus.Error.Disconnected"
                    | "org.freedesktop.DBus.Error.ServiceUnknown"
                    | "org.freedesktop.DBus.Error.NameHasNoOwner"
            ),
            zbus::Error::FDO(error) => matches!(
                error.as_ref(),
                zbus::fdo::Error::NoReply(_)
                    | zbus::fdo::Error::Timeout(_)
                    | zbus::fdo::Error::Disconnected(_)
                    | zbus::fdo::Error::ServiceUnknown(_)
                    | zbus::fdo::Error::NameHasNoOwner(_)
            ),
            _ => false,
        },
    }
}

/// Static coordinator for the two negotiated inputs and six concrete Client resources.
///
/// The caller owns the sole current plan and cancellation token. This type has no mailbox,
/// dynamic registry, resource-erased payload, or background command queue.
pub(crate) struct SnapshotReconciler {
    gateway: GatewayReconciler,
    binding_input: Arc<BindingInputProvider>,
    presentation: Arc<presentation::Presentation>,
    binding: BindingReconciler,
    runtime: RuntimeReconciler,
    session: SessionReconciler,
    home: HomeReconciler,
    power: PowerReconciler,
    caddy: Caddy,
}

/// Complete Server snapshot after all wire-level validation and parsing has succeeded.
#[derive(PartialEq)]
pub(crate) struct ValidatedSnapshot {
    binding_intent: Option<ValidatedBindingIntent>,
    gateway_target: ValidatedGatewayTarget,
    binding_target: ValidatedBindingTarget,
    runtime_origin: String,
    session_target: ValidatedSessionTarget,
    home_epoch: Option<u64>,
    power_target: ValidatedPowerTarget,
}

impl SnapshotReconciler {
    /// Builds the fixed production resource graph and publishes its Device1 service.
    pub(crate) async fn production(
        gateway_hostname: String,
        connection: zbus::Connection,
    ) -> Result<Self, SnapshotError> {
        if !Path::new("/var/lib/natsume/state").is_dir() {
            return Err(SnapshotError::Artifact);
        }
        let caddy = Caddy::production(gateway_hostname);
        let gateway = GatewayReconciler::production();
        let binding_input = Arc::new(BindingInputProvider::production());
        let session = SessionReconciler::production(connection.clone(), Arc::clone(&binding_input));
        let home = HomeReconciler::production(connection.clone());
        let power = PowerReconciler::production(connection.clone());
        DeviceService::start(&connection, Arc::clone(&binding_input)).await?;
        let presentation = Arc::new(presentation::Presentation::new(
            "/var/lib/natsume/state/waiting.json".into(),
            "/var/lib/natsume-display".into(),
            Arc::clone(&binding_input),
        ));
        Ok(Self {
            gateway,
            presentation,
            binding_input,
            binding: BindingReconciler::production(),
            runtime: RuntimeReconciler::production(),
            session,
            home,
            power,
            caddy,
        })
    }

    pub(crate) fn configure_presentation(&self, scope: String) -> Result<(), SnapshotError> {
        self.presentation.configure(scope)
    }

    pub(crate) fn start_logo_downloads(
        &self,
        client: reqwest::Client,
        origin: String,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(Arc::clone(&self.presentation).run(client, origin))
    }

    pub(crate) fn accept_presentation(
        &self,
        team: Option<natsume_local_control_api::WaitingTeam>,
    ) -> Result<(), SnapshotError> {
        self.presentation.accept(team)
    }

    pub(crate) fn presentation_offline(&self) {
        if let Err(error) = self.presentation.disconnect() {
            tracing::error!(%error, "Waiting offline status could not be published");
        }
    }

    /// Waits until a durable local Binding submission may change the complete Client snapshot.
    pub(crate) async fn changed(&self) {
        self.binding_input.changed().await;
    }

    /// Atomically fences Binding authority to one newly received target plan.
    pub(crate) fn begin_plan(&self, plan: &CancellationToken) -> Result<(), SnapshotError> {
        self.binding_input.begin_plan(plan)
    }

    pub(crate) fn refresh_plan(&self, plan: &CancellationToken) -> Result<(), SnapshotError> {
        self.binding_input.refresh_plan(plan)
    }

    /// Revokes all Binding submission authority before a plan is awaited.
    pub(crate) fn end_plan(&self) -> Result<(), SnapshotError> {
        self.binding_input.end_plan()
    }

    /// Applies one complete validated Server projection and returns a freshly sampled snapshot.
    pub(crate) async fn reconcile(
        &self,
        snapshot: &ValidatedSnapshot,
        cancellation: CancellationToken,
    ) -> Result<ReconcileOutcome<ClientStateSnapshot>, SnapshotError> {
        let (local_before, configuration_before) =
            self.inspect_and_guard(snapshot, &cancellation).await?;
        let configuration =
            self.apply_configuration(snapshot, configuration_before, &cancellation)?;
        let maintenance = self
            .reconcile_maintenance(snapshot, local_before, &cancellation)
            .await?;
        let access = self
            .reconcile_access(snapshot, &configuration, maintenance, &cancellation)
            .await?;
        let power_actual = self
            .power
            .reconcile(snapshot.power_target, &cancellation)
            .await?;
        check_cancellation(&cancellation)?;
        Ok(self.build_snapshot(snapshot, configuration, access, &power_actual))
    }

    fn build_snapshot(
        &self,
        snapshot: &ValidatedSnapshot,
        configuration: AppliedConfiguration,
        access: AccessOutcome,
        power_actual: &ReconcileOutcome<PowerControlActualState>,
    ) -> ReconcileOutcome<ClientStateSnapshot> {
        let AppliedConfiguration {
            input,
            gateway_material,
            runtime_actual,
        } = configuration;
        let AccessOutcome {
            session_actual,
            home_actual,
            caddy,
            caddy_failed,
        } = access;
        let gateway_actual = if caddy_failed {
            gateway::recovery_required(&snapshot.gateway_target.credential_id.to_string())
        } else {
            gateway::actual(&snapshot.gateway_target, &gateway_material.actual, &caddy)
        };
        let binding_actual = self.binding.observe(&caddy);
        let retry = gateway_material.retry
            || runtime_actual.retry
            || caddy_failed
            || session_actual.retry
            || home_actual.retry
            || power_actual.retry;
        ReconcileOutcome {
            retry,
            actual: ClientStateSnapshot {
                input: Some(input),
                actual: Some(ActualState {
                    gateway: Some(gateway_actual),
                    binding_access: Some(binding_actual),
                    runtime_config: Some(runtime_actual.actual),
                    session_control: Some(session_actual.actual),
                    home: Some(home_actual.actual),
                    power: Some(power_actual.actual),
                }),
            },
        }
    }

    /// Samples local state and withdraws unsafe access without waiting for a Server target.
    pub(crate) async fn observe(
        &self,
        target: Option<&ValidatedSnapshot>,
    ) -> Result<ClientStateSnapshot, SnapshotError> {
        self.session.maintain_waiting().await?;
        let LocalAccessObservation {
            mut session, home, ..
        } = self.guard_local_access(target).await?;
        if !self.session.binding_foreground_ready().await?
            && self.binding_input.revoke_eligibility()?
        {
            // Revocation selects a new placeholder revision after the sample.
            // The old Binding frame cannot confirm that new presentation.
            session.waiting_ready = false;
        }
        let input = self.observe_input()?;
        let caddy = self.caddy.observe().await;

        Ok(ClientStateSnapshot {
            input: Some(input),
            actual: Some(ActualState {
                gateway: Some(self.gateway.observe(&caddy)),
                binding_access: Some(self.binding.observe(&caddy)),
                runtime_config: Some(self.runtime.observe()),
                session_control: Some(session),
                home: Some(home),
                power: Some(self.power.observe()),
            }),
        })
    }

    fn observe_input(&self) -> Result<ClientInputState, SnapshotError> {
        let gateway_input = self.gateway.observed_input()?;
        let binding_input = self.binding_input.observed_input()?;

        Ok(ClientInputState {
            gateway_credential: gateway_input,
            binding: binding_input,
        })
    }
}

pub(crate) fn snapshot_presentation(
    snapshot: &ServerStateSnapshot,
) -> Option<natsume_local_control_api::WaitingTeam> {
    presentation::from_snapshot(snapshot)
}

pub(crate) fn validate_server_snapshot(
    snapshot: ServerStateSnapshot,
) -> Result<ValidatedSnapshot, SnapshotError> {
    let intent = snapshot
        .intent
        .ok_or(SnapshotError::InvalidServerSnapshot)?;
    let target = snapshot
        .target
        .ok_or(SnapshotError::InvalidServerSnapshot)?;
    let gateway_intent = intent
        .gateway_credential
        .ok_or(SnapshotError::InvalidServerSnapshot)?;
    let gateway_intent = canonical_uuid_v7(&gateway_intent.credential_id)
        .ok_or(SnapshotError::InvalidServerSnapshot)?;
    let gateway_target = target
        .gateway
        .and_then(gateway::validate_target)
        .ok_or(SnapshotError::InvalidServerSnapshot)?;
    if gateway_target.credential_id != gateway_intent {
        return Err(SnapshotError::InvalidServerSnapshot);
    }
    let binding_intent = match intent.binding {
        Some(intent) => {
            Some(binding::validate_intent(intent).ok_or(SnapshotError::InvalidServerSnapshot)?)
        }
        None => None,
    };
    let binding_target = target
        .binding_access
        .and_then(binding::validate_target)
        .ok_or(SnapshotError::InvalidServerSnapshot)?;
    if binding_target.bound.is_some() == binding_intent.is_some() {
        return Err(SnapshotError::InvalidServerSnapshot);
    }
    let runtime_target = target
        .runtime_config
        .ok_or(SnapshotError::InvalidServerSnapshot)?;
    if !is_canonical_https_origin(&runtime_target.domjudge_origin) {
        return Err(SnapshotError::InvalidServerSnapshot);
    }
    let session_target = target
        .session_control
        .and_then(session::validate_target)
        .ok_or(SnapshotError::InvalidServerSnapshot)?;
    let home_target = target.home.ok_or(SnapshotError::InvalidServerSnapshot)?;
    if home_target.reset_epoch.is_some_and(invalid_epoch) {
        return Err(SnapshotError::InvalidServerSnapshot);
    }
    let power_target = target
        .power
        .and_then(power::validate_target)
        .ok_or(SnapshotError::InvalidServerSnapshot)?;
    Ok(ValidatedSnapshot {
        binding_intent,
        gateway_target,
        binding_target,
        runtime_origin: runtime_target.domjudge_origin,
        session_target,
        home_epoch: home_target.reset_epoch,
        power_target,
    })
}

pub(super) fn invalid_epoch(epoch: u64) -> bool {
    epoch == 0 || epoch > i64::MAX.cast_unsigned()
}

pub(super) fn check_cancellation(cancellation: &CancellationToken) -> Result<(), SnapshotError> {
    if cancellation.is_cancelled() {
        Err(SnapshotError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests;
