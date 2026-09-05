use std::{path::Path, sync::Arc};

use natsume_device_protocol::generated::{
    ActualState, ClientInputState, ClientStateSnapshot, HomeActualState, HomeState,
    RuntimeConfigActualState, RuntimeConfigState, ServerStateSnapshot, SessionControlActualState,
    SessionState,
};
use snafu::Snafu;
use tokio_util::sync::CancellationToken;

use crate::canonical_uuid_v7;

mod binding;
mod caddy;
mod gateway;
mod home;
mod runtime;
mod session;

use binding::{
    BindingInputProvider, BindingReconciler, DeviceService, ValidatedBindingIntent,
    ValidatedBindingTarget,
};
use caddy::{Caddy, CaddyObservation};
use gateway::{GatewayMaterialState, GatewayReconciler, ValidatedGatewayTarget};
use home::HomeReconciler;
use runtime::{RuntimeReconciler, is_canonical_https_origin};
use session::{SessionReconciler, ValidatedSessionTarget};

/// Redacted failure at the Client snapshot boundary.
#[derive(Debug, Snafu)]
pub(crate) enum SnapshotError {
    #[snafu(display("Server state snapshot is invalid"))]
    InvalidServerSnapshot,

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

/// Static coordinator for the two negotiated inputs and five concrete Client resources.
///
/// The caller owns the sole current plan and cancellation token. This type has no mailbox,
/// dynamic registry, resource-erased payload, or background command queue.
pub(crate) struct SnapshotReconciler {
    gateway: GatewayReconciler,
    binding_input: Arc<BindingInputProvider>,
    binding: BindingReconciler,
    runtime: RuntimeReconciler,
    session: SessionReconciler,
    home: HomeReconciler,
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
        let session = SessionReconciler::production(connection.clone());
        let home = HomeReconciler::production(connection.clone());
        DeviceService::start(&connection, Arc::clone(&binding_input)).await?;
        Ok(Self {
            gateway,
            binding_input,
            binding: BindingReconciler::production(),
            runtime: RuntimeReconciler::production(),
            session,
            home,
            caddy,
        })
    }

    /// Waits until a durable local Binding submission may change the complete Client snapshot.
    pub(crate) async fn changed(&self) {
        self.binding_input.changed().await;
    }

    /// Removes local access whenever no active Control lease authorizes it.
    pub(crate) async fn deactivate(&self) -> Result<(), SnapshotError> {
        let material = self.gateway.active_material();
        self.caddy
            .ensure_blocked(material.as_ref(), &CancellationToken::new())
            .await
            .map(|_| ())
    }

    /// Atomically fences Binding authority to one newly received target plan.
    pub(crate) fn begin_plan(&self, plan: &CancellationToken) -> Result<(), SnapshotError> {
        self.binding_input.begin_plan(plan)
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
    ) -> Result<ClientStateSnapshot, SnapshotError> {
        check_cancellation(&cancellation)?;
        let gateway_before = self.gateway.current_material(&snapshot.gateway_target);
        let runtime = self.runtime.observe();
        let origin_applied = applied_runtime_origin(&snapshot.runtime_origin, &runtime).is_some();
        let binding_is_applied = self.binding.is_applied(&snapshot.binding_target);
        let current_caddy = if origin_applied && binding_is_applied {
            self.current_caddy(snapshot, &gateway_before).await
        } else {
            None
        };
        check_cancellation(&cancellation)?;
        let blocked_caddy = if current_caddy.is_none() {
            Some(self.caddy.ensure_blocked(None, &cancellation).await?)
        } else {
            None
        };
        check_cancellation(&cancellation)?;
        let gateway_input = Some(self.gateway.current_input(&snapshot.gateway_target)?);
        let binding_input = if let Some(binding_intent) = snapshot.binding_intent.clone() {
            self.binding_input
                .current_input(&cancellation, binding_intent)?
        } else {
            self.binding_input.clear_intent(&cancellation)?;
            None
        };
        check_cancellation(&cancellation)?;
        let gateway_material = match gateway_before {
            GatewayMaterialState::RecoveryRequired => self
                .gateway
                .reconcile(&snapshot.gateway_target, &cancellation)?,
            current => current,
        };
        check_cancellation(&cancellation)?;
        let runtime_actual = if origin_applied {
            runtime
        } else {
            self.runtime
                .reconcile(&snapshot.runtime_origin, &cancellation)?
        };
        check_cancellation(&cancellation)?;
        if !binding_is_applied {
            self.binding
                .reconcile(&snapshot.binding_target, &cancellation)?;
        }
        check_cancellation(&cancellation)?;
        let (caddy, caddy_failed) = if let Some(caddy) = current_caddy {
            (caddy, false)
        } else {
            self.load_caddy(
                &gateway_material,
                &snapshot.binding_target,
                &snapshot.runtime_origin,
                &runtime_actual,
                blocked_caddy,
                &cancellation,
            )
            .await?
        };
        check_cancellation(&cancellation)?;
        let session_actual = self
            .session
            .reconcile(&snapshot.session_target, &cancellation)
            .await?;
        check_cancellation(&cancellation)?;
        let home_actual = self
            .home
            .reconcile(snapshot.home_epoch, &cancellation)
            .await?;
        self.binding_input.set_eligible(
            &cancellation,
            binding_input_is_eligible(&snapshot.binding_target, &session_actual, &home_actual),
        )?;
        check_cancellation(&cancellation)?;
        let gateway_actual = if caddy_failed {
            gateway::recovery_required(&snapshot.gateway_target.credential_id.to_string())
        } else {
            gateway::actual(&snapshot.gateway_target, &gateway_material, &caddy)
        };
        let binding_actual = self.binding.observe(&caddy);
        Ok(ClientStateSnapshot {
            input: Some(ClientInputState {
                gateway_credential: gateway_input,
                binding: binding_input,
            }),
            actual: Some(ActualState {
                gateway: Some(gateway_actual),
                binding_access: Some(binding_actual),
                runtime_config: Some(runtime_actual),
                session_control: Some(session_actual),
                home: Some(home_actual),
            }),
        })
    }

    async fn current_caddy(
        &self,
        snapshot: &ValidatedSnapshot,
        gateway: &GatewayMaterialState,
    ) -> Option<CaddyObservation> {
        let caddy = match gateway {
            GatewayMaterialState::Restoring => self.caddy.current_blocked(None).await,
            GatewayMaterialState::RecoveryRequired => None,
            GatewayMaterialState::Available(material) => {
                if let Some(bound) = snapshot.binding_target.bound.as_ref() {
                    self.caddy
                        .current_ready(
                            material,
                            &snapshot.runtime_origin,
                            &bound.context,
                            bound.password.as_str(),
                        )
                        .await
                } else {
                    self.caddy.current_blocked(Some(material)).await
                }
            }
        };
        match gateway {
            GatewayMaterialState::Available(material) => caddy.filter(|caddy| {
                caddy.gateway_leaf_sha256.as_deref() == Some(material.leaf_sha256.as_slice())
            }),
            GatewayMaterialState::Restoring | GatewayMaterialState::RecoveryRequired => caddy,
        }
    }

    async fn load_caddy(
        &self,
        gateway: &GatewayMaterialState,
        binding_target: &ValidatedBindingTarget,
        runtime_origin: &str,
        runtime_actual: &RuntimeConfigActualState,
        blocked_caddy: Option<CaddyObservation>,
        cancellation: &CancellationToken,
    ) -> Result<(CaddyObservation, bool), SnapshotError> {
        let material = match gateway {
            GatewayMaterialState::Available(material) => Some(material),
            GatewayMaterialState::Restoring | GatewayMaterialState::RecoveryRequired => None,
        };
        let origin = applied_runtime_origin(runtime_origin, runtime_actual);
        if let (Some(material), Some(origin), Some(bound)) =
            (material, origin, binding_target.bound.as_ref())
        {
            match self
                .caddy
                .ensure_ready(
                    material,
                    origin,
                    &bound.context,
                    bound.password.as_str(),
                    cancellation,
                )
                .await
            {
                Ok(caddy)
                    if caddy.gateway_leaf_sha256.as_deref()
                        == Some(material.leaf_sha256.as_slice()) =>
                {
                    return Ok((caddy, false));
                }
                Ok(_) | Err(SnapshotError::Caddy) => {
                    return match self
                        .caddy
                        .ensure_blocked(Some(material), cancellation)
                        .await
                    {
                        Ok(caddy) => Ok((caddy, true)),
                        Err(error) => Err(error),
                    };
                }
                Err(error) => return Err(error),
            }
        }
        if material.is_none()
            && let Some(caddy) = blocked_caddy
        {
            return Ok((caddy, false));
        }
        self.caddy
            .ensure_blocked(material, cancellation)
            .await
            .map(|caddy| (caddy, false))
    }

    /// Re-samples all local artifacts and runtime state into one complete Client projection.
    pub(crate) async fn observe(&self) -> Result<ClientStateSnapshot, SnapshotError> {
        let input = self.observe_input()?;
        let caddy = self.caddy.observe().await;

        Ok(ClientStateSnapshot {
            input: Some(input),
            actual: Some(ActualState {
                gateway: Some(self.gateway.observe(&caddy)),
                binding_access: Some(self.binding.observe(&caddy)),
                runtime_config: Some(self.runtime.observe()),
                session_control: Some(self.session.observe().await?),
                home: Some(self.home.observe().await?),
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

fn binding_input_is_eligible(
    target: &ValidatedBindingTarget,
    session: &SessionControlActualState,
    home: &HomeActualState,
) -> bool {
    target.bound.is_none()
        && matches!(
            SessionState::try_from(session.session_state),
            Ok(SessionState::Active | SessionState::Locked)
        )
        && home.state == i32::from(HomeState::Steady)
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
    Ok(ValidatedSnapshot {
        binding_intent,
        gateway_target,
        binding_target,
        runtime_origin: runtime_target.domjudge_origin,
        session_target,
        home_epoch: home_target.reset_epoch,
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

fn applied_runtime_origin<'a>(
    target_origin: &'a str,
    actual: &'a RuntimeConfigActualState,
) -> Option<&'a str> {
    (actual.state == i32::from(RuntimeConfigState::Applied)
        && actual.applied_domjudge_origin.as_deref() == Some(target_origin))
    .then_some(target_origin)
}

#[cfg(test)]
mod tests {
    use natsume_device_protocol::generated::{
        BindingAccessTarget, BindingNegotiationIntent, ConcreteTargetState,
        GatewayCredentialIntent, GatewayTarget, HomeTarget, LockState, RuntimeConfigTarget,
        ServerIntentState, SessionControlTarget,
    };
    use uuid::Uuid;

    use super::*;

    fn snapshot() -> ServerStateSnapshot {
        let credential_id = Uuid::now_v7().hyphenated().to_string();
        ServerStateSnapshot {
            intent: Some(ServerIntentState {
                gateway_credential: Some(GatewayCredentialIntent {
                    credential_id: credential_id.clone(),
                }),
                binding: Some(BindingNegotiationIntent {
                    negotiation_id: Uuid::now_v7().hyphenated().to_string(),
                    evaluation: None,
                }),
            }),
            target: Some(ConcreteTargetState {
                gateway: Some(GatewayTarget {
                    credential_id,
                    certificate: None,
                }),
                binding_access: Some(BindingAccessTarget { bound: None }),
                runtime_config: Some(RuntimeConfigTarget {
                    domjudge_origin: "https://judge.example".to_owned(),
                }),
                session_control: Some(SessionControlTarget {
                    lock_state: LockState::Unlocked.into(),
                    terminate_epoch: None,
                }),
                home: Some(HomeTarget { reset_epoch: None }),
            }),
        }
    }

    #[test]
    fn complete_server_snapshot_is_validated_before_reconciliation() {
        assert!(validate_server_snapshot(snapshot()).is_ok());

        let mut invalid = snapshot();
        if let Some(gateway) = invalid
            .target
            .as_mut()
            .and_then(|target| target.gateway.as_mut())
        {
            gateway.credential_id = Uuid::now_v7().hyphenated().to_string();
        }
        assert!(matches!(
            validate_server_snapshot(invalid),
            Err(SnapshotError::InvalidServerSnapshot)
        ));
    }

    #[test]
    fn unbound_target_requires_a_negotiation_intent() {
        let mut invalid = snapshot();
        if let Some(intent) = invalid.intent.as_mut() {
            intent.binding = None;
        }

        assert!(matches!(
            validate_server_snapshot(invalid),
            Err(SnapshotError::InvalidServerSnapshot)
        ));
    }

    #[test]
    fn caddy_ready_requires_the_exact_applied_runtime_origin() {
        let target = RuntimeConfigTarget {
            domjudge_origin: "https://judge.example".to_owned(),
        };
        let failed = RuntimeConfigActualState {
            state: RuntimeConfigState::Failed.into(),
            applied_domjudge_origin: Some("https://old.example".to_owned()),
        };
        let wrong = RuntimeConfigActualState {
            state: RuntimeConfigState::Applied.into(),
            applied_domjudge_origin: Some("https://other.example".to_owned()),
        };
        let exact = RuntimeConfigActualState {
            state: RuntimeConfigState::Applied.into(),
            applied_domjudge_origin: Some(target.domjudge_origin.clone()),
        };

        assert_eq!(
            applied_runtime_origin(&target.domjudge_origin, &failed),
            None
        );
        assert_eq!(
            applied_runtime_origin(&target.domjudge_origin, &wrong),
            None
        );
        assert_eq!(
            applied_runtime_origin(&target.domjudge_origin, &exact),
            Some("https://judge.example")
        );
    }

    #[test]
    fn binding_eligibility_requires_a_live_session() {
        let target = ValidatedBindingTarget { bound: None };
        let home = HomeActualState {
            state: HomeState::Steady.into(),
            completed_reset_epoch: None,
        };
        for state in [SessionState::Active, SessionState::Locked] {
            assert!(binding_input_is_eligible(
                &target,
                &SessionControlActualState {
                    session_state: state.into(),
                    completed_terminate_epoch: None,
                },
                &home,
            ));
        }
        for state in [
            SessionState::None,
            SessionState::Starting,
            SessionState::Terminating,
            SessionState::Ambiguous,
            SessionState::Error,
        ] {
            assert!(!binding_input_is_eligible(
                &target,
                &SessionControlActualState {
                    session_state: state.into(),
                    completed_terminate_epoch: None,
                },
                &home,
            ));
        }
    }

    #[test]
    fn cancelled_plan_is_rejected_before_an_effect() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        assert!(matches!(
            check_cancellation(&cancellation),
            Err(SnapshotError::Cancelled)
        ));
    }
}
