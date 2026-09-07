use std::{path::Path, sync::Arc};

use natsume_device_protocol::generated::{
    ActualState, ClientInputState, ClientStateSnapshot, HomeActualState, HomeState,
    RuntimeConfigActualState, RuntimeConfigState, ServerStateSnapshot, SessionControlActualState,
    SessionState,
};
use natsume_local_control_api::ResourceControlError;
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

    /// Revokes Binding input and confirms the loaded data plane is blocked.
    pub(crate) async fn deactivate(&self) -> Result<(), SnapshotError> {
        self.binding_input.revoke_eligibility()?;
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
    #[expect(
        clippy::too_many_lines,
        reason = "keep the fixed resource effect order and retry aggregation visible"
    )]
    pub(crate) async fn reconcile(
        &self,
        snapshot: &ValidatedSnapshot,
        cancellation: CancellationToken,
    ) -> Result<ReconcileOutcome<ClientStateSnapshot>, SnapshotError> {
        check_cancellation(&cancellation)?;
        let (_, home_before, access_before) = self.guard_local_access(Some(snapshot)).await?;
        let gateway_before = self.gateway.current_material(&snapshot.gateway_target);
        let runtime = self.runtime.observe();
        let origin_applied = applied_runtime_origin(&snapshot.runtime_origin, &runtime).is_some();
        let binding_is_applied = self.binding.is_applied(&snapshot.binding_target);
        let current_caddy = if access_before && origin_applied && binding_is_applied {
            self.current_caddy(snapshot, &gateway_before).await
        } else {
            None
        };
        check_cancellation(&cancellation)?;
        if current_caddy.is_none() {
            self.deactivate().await?;
        }
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
            current => ReconcileOutcome::idle(current),
        };
        check_cancellation(&cancellation)?;
        let runtime_actual = if origin_applied {
            ReconcileOutcome::idle(runtime)
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
        let mut session_actual = self
            .session
            .reconcile(&snapshot.session_target, &cancellation)
            .await?;
        if access_before && !local_access_is_allowed(snapshot, &session_actual.actual, &home_before)
        {
            self.deactivate().await?;
        }
        check_cancellation(&cancellation)?;
        let mut home_actual = self
            .home
            .reconcile(snapshot.home_epoch, &cancellation)
            .await?;
        let reconciled_access =
            local_access_is_allowed(snapshot, &session_actual.actual, &home_actual.actual);
        if !reconciled_access {
            self.deactivate().await?;
        }
        check_cancellation(&cancellation)?;
        // Home may stop the display manager after Session reconciliation. Sample
        // both again before opening access, while retaining resource failures.
        let (session_observed, home_observed, observed_access) =
            self.guard_local_access(Some(snapshot)).await?;
        if matches!(
            SessionState::try_from(session_actual.actual.session_state),
            Ok(SessionState::Running)
        ) {
            session_actual.actual = session_observed;
        }
        if home_actual.actual.state == i32::from(HomeState::Steady) {
            home_actual.actual = home_observed;
        }
        let allowed = reconciled_access && observed_access;
        check_cancellation(&cancellation)?;
        let (caddy, caddy_failed) = self
            .load_caddy(
                snapshot,
                &gateway_material.actual,
                &runtime_actual.actual,
                allowed,
                &cancellation,
            )
            .await?;
        self.binding_input.set_eligible(
            &cancellation,
            allowed
                && snapshot.binding_target.bound.is_none()
                && session_actual.actual.waiting_ready
                && session_actual.actual.foreground
                    == i32::from(natsume_device_protocol::generated::SessionForeground::Waiting),
        )?;
        check_cancellation(&cancellation)?;
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
            || home_actual.retry;
        Ok(ReconcileOutcome {
            retry,
            actual: ClientStateSnapshot {
                input: Some(ClientInputState {
                    gateway_credential: gateway_input,
                    binding: binding_input,
                }),
                actual: Some(ActualState {
                    gateway: Some(gateway_actual),
                    binding_access: Some(binding_actual),
                    runtime_config: Some(runtime_actual.actual),
                    session_control: Some(session_actual.actual),
                    home: Some(home_actual.actual),
                }),
            },
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
        snapshot: &ValidatedSnapshot,
        gateway: &GatewayMaterialState,
        runtime_actual: &RuntimeConfigActualState,
        local_access: bool,
        cancellation: &CancellationToken,
    ) -> Result<(CaddyObservation, bool), SnapshotError> {
        let material = match gateway {
            GatewayMaterialState::Available(material) => Some(material),
            GatewayMaterialState::Restoring | GatewayMaterialState::RecoveryRequired => None,
        };
        let origin = applied_runtime_origin(&snapshot.runtime_origin, runtime_actual);
        if let (Some(material), Some(origin), Some(bound)) =
            (material, origin, snapshot.binding_target.bound.as_ref())
            && local_access
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
        self.caddy
            .ensure_blocked(material, cancellation)
            .await
            .map(|caddy| (caddy, false))
    }

    /// Samples current OS facts and closes access before returning unsafe facts or errors.
    /// This path never grants access, so observing an old target cannot reopen it.
    async fn guard_local_access(
        &self,
        target: Option<&ValidatedSnapshot>,
    ) -> Result<(SessionControlActualState, HomeActualState, bool), SnapshotError> {
        let session = self.session.observe().await;
        let session_allowed = match (target, &session) {
            (Some(target), Ok(session)) => session_access_is_allowed(target, session),
            _ => false,
        };
        if !session_allowed {
            self.deactivate().await?;
        }
        let home = self.home.observe().await;
        let allowed = match (target, &session, &home) {
            (Some(target), Ok(session), Ok(home)) => local_access_is_allowed(target, session, home),
            _ => false,
        };
        if session_allowed && !allowed {
            self.deactivate().await?;
        }
        Ok((session?, home?, allowed))
    }

    /// Samples local state and withdraws unsafe access without waiting for a Server target.
    pub(crate) async fn observe(
        &self,
        target: Option<&ValidatedSnapshot>,
    ) -> Result<ClientStateSnapshot, SnapshotError> {
        let (session, home, _) = self.guard_local_access(target).await?;
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

fn local_access_is_allowed(
    target: &ValidatedSnapshot,
    session: &SessionControlActualState,
    home: &HomeActualState,
) -> bool {
    session_access_is_allowed(target, session)
        && home.state == i32::from(HomeState::Steady)
        && home.completed_reset_epoch == target.home_epoch
}

fn session_access_is_allowed(
    target: &ValidatedSnapshot,
    session: &SessionControlActualState,
) -> bool {
    matches!(
        SessionState::try_from(session.session_state),
        Ok(SessionState::Running)
    ) && session.contest_ready
        && target.session_target.termination_is_complete(session)
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
pub(crate) mod tests {
    use natsume_device_protocol::generated::{
        BindingAccessTarget, BindingNegotiationIntent, ConcreteTargetState, ForegroundTarget,
        GatewayCredentialIntent, GatewayTarget, HomeTarget, RuntimeConfigTarget, ServerIntentState,
        SessionControlTarget,
    };
    use uuid::Uuid;

    use super::*;

    #[test]
    fn helper_retry_policy_uses_error_types_and_rejects_unknown_failures() {
        assert!(retryable_control_error(&ResourceControlError::Unavailable(
            "same text".to_owned()
        )));
        assert!(!retryable_control_error(&ResourceControlError::Rejected(
            "same text".to_owned()
        )));
        assert!(retryable_control_error(&ResourceControlError::ZBus(
            zbus::fdo::Error::NoReply("timeout".to_owned()).into()
        )));
        assert!(!retryable_control_error(&ResourceControlError::ZBus(
            zbus::fdo::Error::AccessDenied("denied".to_owned()).into()
        )));
        assert!(!retryable_control_error(&ResourceControlError::ZBus(
            zbus::Error::Failure("unknown".to_owned())
        )));
    }

    use natsume_local_control_api::{
        GraphicalSession, GraphicalSessionObservation, GraphicalSessionState, HomeResetPhase,
        HomeResetProgress, ManagedSessionsObservation, PRIVILEGED1_PATH, ResourceControlError,
        SessionForeground,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    pub(crate) struct HelperState {
        pub(crate) session_state: Option<GraphicalSessionState>,
        pub(crate) foreground: Option<SessionForeground>,
        pub(crate) session_after_home: Option<GraphicalSessionState>,
        pub(crate) require_blocked: Option<std::path::PathBuf>,
        pub(crate) termination_calls: Vec<GraphicalSession>,
        pub(crate) home_calls: Vec<u64>,
        pub(crate) terminate_failures: usize,
        pub(crate) home_failures: usize,
        pub(crate) rejected: bool,
        pub(crate) progress: Option<HomeResetProgress>,
    }

    struct FaultyHelper(Arc<Mutex<HelperState>>);

    #[zbus::interface(name = "org.natsume.Privileged1")]
    impl FaultyHelper {
        #[zbus(name = "QueryManagedSessions")]
        fn query_managed_sessions(&self) -> ManagedSessionsObservation {
            let state = self
                .0
                .lock()
                .unwrap_or_else(|e| panic!("fixture lock: {e}"));
            let lifecycle = state
                .session_state
                .unwrap_or(GraphicalSessionState::Running);
            ManagedSessionsObservation {
                contest: GraphicalSessionObservation {
                    state: lifecycle,
                    session: matches!(
                        lifecycle,
                        GraphicalSessionState::Running
                            | GraphicalSessionState::Starting
                            | GraphicalSessionState::Terminating
                    )
                    .then(|| GraphicalSession {
                        logind_session_id: if state.termination_calls.is_empty() {
                            "c2"
                        } else {
                            "c3"
                        }
                        .to_owned(),
                        boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
                    }),
                    desktop_ready: lifecycle == GraphicalSessionState::Running,
                    locked_hint: false,
                },
                waiting: GraphicalSessionObservation {
                    state: GraphicalSessionState::Running,
                    session: Some(GraphicalSession {
                        logind_session_id: "w1".to_owned(),
                        boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
                    }),
                    desktop_ready: true,
                    locked_hint: false,
                },
                foreground: state.foreground.unwrap_or(SessionForeground::Contest),
            }
        }

        #[zbus(name = "TerminateContestSession")]
        fn terminate_contest_session(
            &mut self,
            session: GraphicalSession,
        ) -> Result<(), ResourceControlError> {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|e| panic!("fixture lock: {e}"));
            state.assert_blocked();
            state.termination_calls.push(session);
            if state.rejected {
                Err(ResourceControlError::Rejected(
                    "invalid session target".to_owned(),
                ))
            } else if state.termination_calls.len() <= state.terminate_failures {
                Err(ResourceControlError::Unavailable(
                    "temporary logind failure".to_owned(),
                ))
            } else {
                Ok(())
            }
        }

        #[zbus(name = "QueryHomeReset")]
        fn query_home_reset(&self) -> Option<HomeResetProgress> {
            self.0
                .lock()
                .unwrap_or_else(|e| panic!("fixture lock: {e}"))
                .progress
                .clone()
        }

        #[zbus(name = "PrepareHomeReset")]
        fn prepare_home_reset(&mut self, epoch: u64) {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|e| panic!("fixture lock: {e}"));
            state.assert_blocked();
            state.progress = Some(HomeResetProgress {
                reset_epoch: epoch,
                phase: HomeResetPhase::Prepared,
            });
        }

        #[zbus(name = "ApplyHomeReset")]
        fn apply_home_reset(&mut self, epoch: u64) -> Result<(), ResourceControlError> {
            self.finish_home(epoch)
        }

        #[zbus(name = "RecoverHomeReset")]
        fn recover_home_reset(&mut self, epoch: u64) -> Result<(), ResourceControlError> {
            self.finish_home(epoch)
        }

        #[zbus(name = "VerifyHomeReset")]
        fn verify_home_reset(&mut self, epoch: u64) -> HomeResetProgress {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|e| panic!("fixture lock: {e}"));
            let phase = if state.home_calls.len() > state.home_failures && !state.rejected {
                HomeResetPhase::Verified
            } else {
                HomeResetPhase::RecoveryRequired
            };
            let progress = HomeResetProgress {
                reset_epoch: epoch,
                phase,
            };
            state.progress = Some(progress.clone());
            progress
        }
    }

    impl FaultyHelper {
        fn finish_home(&self, epoch: u64) -> Result<(), ResourceControlError> {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|e| panic!("fixture lock: {e}"));
            state.assert_blocked();
            state.home_calls.push(epoch);
            if state.rejected {
                Err(ResourceControlError::Rejected("unmanaged mount".to_owned()))
            } else if state.home_calls.len() <= state.home_failures {
                Err(ResourceControlError::Unavailable(
                    "temporary mount failure".to_owned(),
                ))
            } else {
                if let Some(session) = state.session_after_home {
                    state.session_state = Some(session);
                }
                Ok(())
            }
        }
    }

    impl HelperState {
        fn assert_blocked(&self) {
            if let Some(path) = &self.require_blocked {
                let mode = std::fs::read(path).ok().and_then(|encoded| {
                    serde_json::from_slice::<caddy::CaddyModeArtifact>(&encoded).ok()
                });
                assert!(
                    matches!(mode, Some(caddy::CaddyModeArtifact::Blocked { .. })),
                    "Home/terminate effect started without confirmed BLOCKED"
                );
            }
        }
    }

    #[tokio::test]
    async fn helper_owner_replacement_preserves_pending_session_and_home_work()
    -> Result<(), Box<dyn std::error::Error>> {
        use natsume_local_control_api::{PRIVILEGED1_SERVICE, Privileged1Proxy};
        use tokio::time::{Duration, timeout};

        let directory = tempfile::tempdir()?;
        let (mut bus, address) = private_bus(&directory).await?;
        let connection = zbus::connection::Builder::address(address.as_str())?
            .method_timeout(Duration::from_secs(2))
            .build()
            .await?;
        let daemon_owner = connection.unique_name().cloned();
        let helper = Arc::new(Mutex::new(HelperState {
            terminate_failures: 1,
            home_failures: 1,
            ..HelperState::default()
        }));
        let service = register_helper(&address, Arc::clone(&helper)).await?;
        let old_owner = service.unique_name().cloned();
        let session = session::tests::reconciler(&directory, connection.clone());
        let home = home::tests::reconciler(&directory, connection.clone());
        let target = session::validate_target(SessionControlTarget {
            foreground_target: ForegroundTarget::Contest.into(),
            terminate_epoch: Some(7),
        })
        .ok_or("invalid fixture target")?;
        let cancellation = CancellationToken::new();

        let pending = session.reconcile(&target, &cancellation).await?;
        assert!(pending.retry);
        assert_eq!(pending.actual.completed_terminate_epoch, None);
        let pending = home.reconcile(Some(8), &cancellation).await?;
        assert!(pending.retry);
        assert_eq!(pending.actual.completed_reset_epoch, None);
        let session_path = directory.path().join("session-completion.json");
        let persisted_pending = std::fs::read(&session_path)?;

        service.close().await?;
        let bus_proxy = zbus::fdo::DBusProxy::new(&connection).await?;
        timeout(Duration::from_secs(2), async {
            while bus_proxy
                .name_has_owner(PRIVILEGED1_SERVICE.try_into()?)
                .await?
            {
                tokio::task::yield_now().await;
            }
            Ok::<_, zbus::Error>(())
        })
        .await??;
        let unavailable = session.reconcile(&target, &cancellation).await?;
        assert!(unavailable.retry);
        assert_eq!(unavailable.actual.completed_terminate_epoch, None);
        let unavailable = home.reconcile(Some(8), &cancellation).await?;
        assert!(unavailable.retry);
        assert_eq!(unavailable.actual.completed_reset_epoch, None);
        assert_eq!(
            session.observe().await?.session_state,
            i32::from(SessionState::Terminating)
        );
        assert_eq!(
            home.observe().await?.state,
            i32::from(HomeState::RecoveryRequired)
        );
        assert_eq!(std::fs::read(&session_path)?, persisted_pending);
        assert!(!directory.path().join("home-completion.json").exists());

        // A new bus owner reads the same Helper-owned durable progress; it may
        // observe a new graphical session, but the pending operation stays on c2.
        let replacement = register_helper(&address, Arc::clone(&helper)).await?;
        assert_ne!(replacement.unique_name(), old_owner.as_ref());
        assert_eq!(connection.unique_name(), daemon_owner.as_ref());
        let fresh = Privileged1Proxy::new(&connection)
            .await?
            .query_managed_sessions()
            .await?;
        assert_eq!(
            fresh
                .contest
                .session
                .ok_or("missing replacement session")?
                .logind_session_id,
            "c3"
        );
        let completed = session.reconcile(&target, &cancellation).await?;
        assert!(!completed.retry);
        assert_eq!(completed.actual.completed_terminate_epoch, Some(7));
        let completed = home.reconcile(Some(8), &cancellation).await?;
        assert!(!completed.retry);
        assert_eq!(completed.actual.completed_reset_epoch, Some(8));
        assert_eq!(session.observe().await?.completed_terminate_epoch, Some(7));
        assert_eq!(home.observe().await?.completed_reset_epoch, Some(8));
        {
            let state = helper
                .lock()
                .unwrap_or_else(|error| panic!("fixture lock: {error}"));
            assert_eq!(state.termination_calls.len(), 2);
            assert_eq!(state.termination_calls[0], state.termination_calls[1]);
            assert_eq!(state.termination_calls[1].logind_session_id, "c2");
            assert_eq!(state.home_calls, [8, 8]);
        }
        replacement.close().await?;
        bus.kill().await?;
        Ok(())
    }

    async fn private_bus(
        directory: &tempfile::TempDir,
    ) -> Result<(tokio::process::Child, String), Box<dyn std::error::Error>> {
        use std::process::Stdio;
        use tokio::{
            io::{AsyncBufReadExt as _, BufReader},
            time::{Duration, timeout},
        };

        let mut bus = tokio::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .arg(format!(
                "--address=unix:path={}",
                directory.path().join("bus.sock").display()
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let mut output = BufReader::new(bus.stdout.take().ok_or("missing bus stdout")?);
        let mut address = String::new();
        timeout(Duration::from_secs(5), output.read_line(&mut address)).await??;
        if address.trim().is_empty() {
            return Err("private bus did not publish its address".into());
        }
        Ok((bus, address.trim().to_owned()))
    }

    async fn register_helper(
        address: &str,
        helper: Arc<Mutex<HelperState>>,
    ) -> Result<zbus::Connection, zbus::Error> {
        zbus::connection::Builder::address(address)?
            .name(natsume_local_control_api::PRIVILEGED1_SERVICE)?
            .serve_at(PRIVILEGED1_PATH, FaultyHelper(helper))?
            .build()
            .await
    }

    pub(crate) struct Fixture {
        pub(crate) snapshots: Arc<SnapshotReconciler>,
        pub(crate) helper: Arc<Mutex<HelperState>>,
        pub(crate) directory: tempfile::TempDir,
        _service: zbus::Connection,
        caddy_task: tokio::task::JoinHandle<()>,
        gateway_task: Option<tokio::task::JoinHandle<()>>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.caddy_task.abort();
            if let Some(task) = &self.gateway_task {
                task.abort();
            }
        }
    }

    pub(crate) async fn fixture(state: HelperState) -> Result<Fixture, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let helper = Arc::new(Mutex::new(state));
        let (server_stream, client_stream) = tokio::net::UnixStream::pair()?;
        let server = zbus::connection::Builder::unix_stream(server_stream)
            .server(zbus::Guid::generate())?
            .p2p()
            .serve_at(PRIVILEGED1_PATH, FaultyHelper(Arc::clone(&helper)))?
            .build();
        let client = zbus::connection::Builder::unix_stream(client_stream)
            .p2p()
            .build();
        let (server, connection) = tokio::try_join!(server, client)?;
        let (caddy, caddy_task) = caddy::tests::fixture(&directory)?;
        let snapshots = Arc::new(SnapshotReconciler {
            gateway: gateway::tests::reconciler(&directory),
            binding_input: Arc::new(binding::tests::input_provider(&directory)),
            binding: binding::tests::reconciler(&directory),
            runtime: runtime::tests::reconciler(&directory),
            session: session::tests::reconciler(&directory, connection.clone()),
            home: home::tests::reconciler(&directory, connection),
            caddy,
        });
        Ok(Fixture {
            snapshots,
            helper,
            directory,
            _service: server,
            caddy_task,
            gateway_task: None,
        })
    }

    async fn ready_fixture() -> Result<(Fixture, ValidatedSnapshot), Box<dyn std::error::Error>> {
        use natsume_device_protocol::generated::{GatewayCertificateGrant, GatewayState};
        use rustls_pki_types::pem::PemObject as _;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut fixture = fixture(HelperState::default()).await?;
        let mut target = validate_server_snapshot(snapshot())?;
        let resources = Arc::get_mut(&mut fixture.snapshots).ok_or("fixture already shared")?;
        resources.gateway.current_input(&target.gateway_target)?;
        let key_pem = zeroize::Zeroizing::new(std::fs::read(
            fixture
                .directory
                .path()
                .join("gateway")
                .join(target.gateway_target.credential_id.to_string())
                .join("key.pem"),
        )?);
        let key = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(
            &rustls_pki_types::PrivatePkcs8KeyDer::from_pem_slice(&key_pem)?,
            &rcgen::PKCS_ECDSA_P256_SHA256,
        )?;
        let (certificate, task) =
            caddy::tests::serve_gateway(&mut resources.caddy, &fixture.directory, &key).await?;
        fixture.gateway_task = Some(task);
        target.gateway_target = gateway::validate_target(GatewayTarget {
            credential_id: target.gateway_target.credential_id.to_string(),
            certificate: Some(GatewayCertificateGrant {
                gateway_leaf_der: certificate.der().to_vec(),
            }),
        })
        .ok_or("fixture certificate")?;
        target.binding_target.bound = Some(binding::ValidatedBoundTarget {
            context: binding::ValidatedBindingContext {
                binding_id: Uuid::now_v7().to_string(),
                account_id: Uuid::now_v7().to_string(),
                seat_code: "A-01".to_owned(),
                domjudge_username: "team-alpha".to_owned(),
                credential_revision: 1,
            },
            password: zeroize::Zeroizing::new("password-canary".to_owned()),
        });
        target.binding_intent = None;
        let fence = CancellationToken::new();
        resources.begin_plan(&fence)?;
        let actual = resources
            .reconcile(&target, fence)
            .await?
            .actual
            .actual
            .ok_or("missing actual")?;
        assert_eq!(
            actual.gateway.ok_or("missing gateway")?.state,
            i32::from(GatewayState::Ready)
        );
        fixture
            .helper
            .lock()
            .map_err(|_| "fixture lock")?
            .require_blocked = Some(fixture.directory.path().join("caddy-mode.json"));
        Ok((fixture, target))
    }

    pub(crate) fn snapshot() -> ServerStateSnapshot {
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
                    foreground_target: ForegroundTarget::Contest.into(),
                    terminate_epoch: None,
                }),
                home: Some(HomeTarget { reset_epoch: None }),
            }),
        }
    }

    #[tokio::test]
    async fn observation_revokes_binding_and_loaded_access_without_a_new_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = fixture(HelperState::default()).await?;
        let wire = snapshot();
        let negotiation = wire
            .intent
            .as_ref()
            .and_then(|intent| intent.binding.as_ref())
            .ok_or("missing intent")?
            .negotiation_id
            .clone();
        let target = validate_server_snapshot(wire)?;
        let fence = CancellationToken::new();
        fixture.snapshots.begin_plan(&fence)?;
        fixture.snapshots.reconcile(&target, fence).await?;
        // The Caddy fixture exercises real mode/config persistence and admin I/O.
        // Seed a previously loaded READY mode without requiring a public TLS port.
        std::fs::write(
            fixture.directory.path().join("caddy-mode.json"),
            serde_json::to_vec(&caddy::CaddyModeArtifact::Ready {
                format_version: 1,
                credential_id: target.gateway_target.credential_id.to_string(),
                domjudge_origin: target.runtime_origin.clone(),
                binding: binding::ValidatedBindingContext {
                    binding_id: Uuid::now_v7().to_string(),
                    account_id: Uuid::now_v7().to_string(),
                    seat_code: "A-01".to_owned(),
                    domjudge_username: "team-alpha".to_owned(),
                    credential_revision: 1,
                },
            })?,
        )?;
        fixture
            .helper
            .lock()
            .map_err(|_| "fixture lock")?
            .session_state = Some(GraphicalSessionState::Ambiguous);
        fixture.snapshots.observe(Some(&target)).await?;
        assert!(matches!(
            fixture
                .snapshots
                .binding_input
                .submit(&negotiation, 1, "A-01"),
            Err(SnapshotError::StaleLocalInput)
        ));
        assert!(matches!(
            fixture.snapshots.caddy.observe().await.mode,
            Some(caddy::CaddyModeArtifact::Blocked { .. })
        ));
        Ok(())
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
    fn local_access_requires_a_ready_contest_in_either_foreground() {
        let target = validate_server_snapshot(snapshot())
            .unwrap_or_else(|error| panic!("fixture target: {error}"));
        let home = HomeActualState {
            state: HomeState::Steady.into(),
            completed_reset_epoch: None,
        };
        for foreground in [
            natsume_device_protocol::generated::SessionForeground::Contest,
            natsume_device_protocol::generated::SessionForeground::Waiting,
        ] {
            assert!(local_access_is_allowed(
                &target,
                &SessionControlActualState {
                    contest_ready: true,
                    waiting_ready: false,
                    foreground: foreground.into(),
                    session_state: SessionState::Running.into(),
                    completed_terminate_epoch: None,
                },
                &home,
            ));
        }
        assert!(!local_access_is_allowed(
            &target,
            &SessionControlActualState {
                session_state: SessionState::Running.into(),
                ..SessionControlActualState::default()
            },
            &home
        ));
        for state in [
            SessionState::Unspecified,
            SessionState::None,
            SessionState::Starting,
            SessionState::Terminating,
            SessionState::Ambiguous,
            SessionState::Error,
        ] {
            assert!(!local_access_is_allowed(
                &target,
                &SessionControlActualState {
                    contest_ready: true,
                    waiting_ready: false,
                    foreground: natsume_device_protocol::generated::SessionForeground::Contest
                        .into(),
                    session_state: state.into(),
                    completed_terminate_epoch: None,
                },
                &home,
            ));
        }
    }

    #[test]
    fn local_access_requires_steady_home_and_exact_completed_epochs() -> Result<(), SnapshotError> {
        let mut target = validate_server_snapshot(snapshot())?;
        target.home_epoch = Some(7);
        target.session_target = session::validate_target(SessionControlTarget {
            foreground_target: ForegroundTarget::Contest.into(),
            terminate_epoch: Some(8),
        })
        .ok_or(SnapshotError::InvalidServerSnapshot)?;
        for home_epoch in [None, Some(6), Some(7), Some(9)] {
            for session_epoch in [None, Some(7), Some(8), Some(9)] {
                for state in [
                    HomeState::Unspecified,
                    HomeState::Resetting,
                    HomeState::RecoveryRequired,
                    HomeState::Steady,
                ] {
                    assert_eq!(
                        local_access_is_allowed(
                            &target,
                            &SessionControlActualState {
                                contest_ready: true,
                                waiting_ready: false,
                                foreground:
                                    natsume_device_protocol::generated::SessionForeground::Contest
                                        .into(),
                                session_state: SessionState::Running.into(),
                                completed_terminate_epoch: session_epoch,
                            },
                            &HomeActualState {
                                state: state.into(),
                                completed_reset_epoch: home_epoch
                            }
                        ),
                        home_epoch == Some(7)
                            && session_epoch == Some(8)
                            && state == HomeState::Steady
                    );
                }
            }
        }
        assert!(!local_access_is_allowed(
            &target,
            &SessionControlActualState {
                contest_ready: true,
                waiting_ready: false,
                foreground: natsume_device_protocol::generated::SessionForeground::Contest.into(),
                session_state: 999,
                completed_terminate_epoch: Some(8)
            },
            &HomeActualState {
                state: HomeState::Steady.into(),
                completed_reset_epoch: Some(7)
            }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn loaded_ready_is_blocked_on_local_failure_and_restored_by_current_plan()
    -> Result<(), Box<dyn std::error::Error>> {
        let (fixture, target) = ready_fixture().await?;
        let binding = std::fs::read(fixture.directory.path().join("binding-assignment.json"))?;
        for state in [
            GraphicalSessionState::None,
            GraphicalSessionState::Ambiguous,
        ] {
            fixture
                .helper
                .lock()
                .map_err(|_| "fixture lock")?
                .session_state = Some(state);
            fixture.snapshots.observe(Some(&target)).await?;
            assert!(matches!(
                fixture.snapshots.caddy.observe().await.mode,
                Some(caddy::CaddyModeArtifact::Blocked { .. })
            ));
            fixture
                .helper
                .lock()
                .map_err(|_| "fixture lock")?
                .session_state = Some(GraphicalSessionState::Running);
            // Observation has no authority to grant access again.
            fixture.snapshots.observe(Some(&target)).await?;
            assert!(matches!(
                fixture.snapshots.caddy.observe().await.mode,
                Some(caddy::CaddyModeArtifact::Blocked { .. })
            ));
            let fence = CancellationToken::new();
            fixture.snapshots.begin_plan(&fence)?;
            fixture.snapshots.reconcile(&target, fence).await?;
            assert!(matches!(
                fixture.snapshots.caddy.observe().await.mode,
                Some(caddy::CaddyModeArtifact::Ready { .. })
            ));
            assert_eq!(
                std::fs::read(fixture.directory.path().join("binding-assignment.json"))?,
                binding
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn pending_epochs_block_ready_before_helper_effects_and_wait_for_completion()
    -> Result<(), Box<dyn std::error::Error>> {
        let (fixture, mut target) = ready_fixture().await?;
        target.home_epoch = Some(7);
        target.session_target = session::validate_target(SessionControlTarget {
            foreground_target: ForegroundTarget::Contest.into(),
            terminate_epoch: Some(8),
        })
        .ok_or(SnapshotError::InvalidServerSnapshot)?;
        {
            let mut state = fixture.helper.lock().map_err(|_| "fixture lock")?;
            state.terminate_failures = 1;
            state.home_failures = 1;
        }
        for complete in [false, true] {
            let fence = CancellationToken::new();
            fixture.snapshots.begin_plan(&fence)?;
            let outcome = fixture.snapshots.reconcile(&target, fence).await?;
            let actual = outcome.actual.actual.ok_or("missing actual")?;
            assert_eq!(outcome.retry, !complete);
            assert_eq!(
                actual.home.ok_or("missing Home")?.completed_reset_epoch,
                complete.then_some(7)
            );
            assert_eq!(
                actual
                    .session_control
                    .ok_or("missing Session")?
                    .completed_terminate_epoch,
                complete.then_some(8)
            );
            assert_eq!(
                matches!(
                    fixture.snapshots.caddy.observe().await.mode,
                    Some(caddy::CaddyModeArtifact::Ready { .. })
                ),
                complete
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn home_effect_cannot_reopen_access_from_the_pre_reset_session_observation()
    -> Result<(), Box<dyn std::error::Error>> {
        let (fixture, mut target) = ready_fixture().await?;
        target.home_epoch = Some(7);
        fixture
            .helper
            .lock()
            .map_err(|_| "fixture lock")?
            .session_after_home = Some(GraphicalSessionState::None);
        let fence = CancellationToken::new();
        fixture.snapshots.begin_plan(&fence)?;
        let actual = fixture
            .snapshots
            .reconcile(&target, fence)
            .await?
            .actual
            .actual
            .ok_or("missing actual")?;
        assert_eq!(
            actual.home.ok_or("missing Home")?.state,
            i32::from(HomeState::Steady)
        );
        assert_eq!(
            actual
                .session_control
                .ok_or("missing Session")?
                .session_state,
            i32::from(SessionState::None)
        );
        assert!(matches!(
            fixture.snapshots.caddy.observe().await.mode,
            Some(caddy::CaddyModeArtifact::Blocked { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn failed_blocking_aborts_before_any_home_or_terminate_effect()
    -> Result<(), Box<dyn std::error::Error>> {
        let (fixture, mut target) = ready_fixture().await?;
        target.home_epoch = Some(7);
        target.session_target = session::validate_target(SessionControlTarget {
            foreground_target: ForegroundTarget::Contest.into(),
            terminate_epoch: Some(8),
        })
        .ok_or(SnapshotError::InvalidServerSnapshot)?;
        std::fs::write(
            fixture.directory.path().join("caddy-fixture"),
            "#!/bin/sh\nexit 1\n",
        )?;
        let fence = CancellationToken::new();
        fixture.snapshots.begin_plan(&fence)?;
        assert!(matches!(
            fixture.snapshots.reconcile(&target, fence).await,
            Err(SnapshotError::Caddy)
        ));
        {
            let mut state = fixture.helper.lock().map_err(|_| "fixture lock")?;
            assert!(
                state.termination_calls.is_empty()
                    && state.home_calls.is_empty()
                    && state.progress.is_none()
            );
            state.session_state = Some(GraphicalSessionState::Ambiguous);
        }
        assert!(matches!(
            fixture.snapshots.observe(Some(&target)).await,
            Err(SnapshotError::Caddy)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn foreground_and_repeated_targets_keep_ready_without_reloading_caddy()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::MetadataExt as _;
        let (fixture, mut target) = ready_fixture().await?;
        let mode = fixture.directory.path().join("caddy-mode.json");
        let inode = std::fs::metadata(&mode)?.ino();
        for foreground in [ForegroundTarget::Contest, ForegroundTarget::Waiting] {
            fixture
                .helper
                .lock()
                .map_err(|_| "fixture lock")?
                .foreground = Some(if foreground == ForegroundTarget::Waiting {
                SessionForeground::Waiting
            } else {
                SessionForeground::Contest
            });
            target.session_target = session::validate_target(SessionControlTarget {
                foreground_target: foreground.into(),
                terminate_epoch: None,
            })
            .ok_or("fixture Session target")?;
            let fence = CancellationToken::new();
            fixture.snapshots.begin_plan(&fence)?;
            fixture.snapshots.reconcile(&target, fence).await?;
            assert!(matches!(
                fixture.snapshots.caddy.observe().await.mode,
                Some(caddy::CaddyModeArtifact::Ready { .. })
            ));
            assert_eq!(std::fs::metadata(&mode)?.ino(), inode);
        }
        Ok(())
    }

    #[tokio::test]
    async fn observation_cannot_revoke_replacement_plan_ownership_or_reopen_old_access()
    -> Result<(), Box<dyn std::error::Error>> {
        let (fixture, mut target) = ready_fixture().await?;
        let old = CancellationToken::new();
        fixture.snapshots.begin_plan(&old)?;
        let replacement = CancellationToken::new();
        old.cancel();
        fixture.snapshots.begin_plan(&replacement)?;
        fixture
            .helper
            .lock()
            .map_err(|_| "fixture lock")?
            .session_state = Some(GraphicalSessionState::Ambiguous);
        fixture.snapshots.observe(Some(&target)).await?;
        fixture
            .helper
            .lock()
            .map_err(|_| "fixture lock")?
            .session_state = Some(GraphicalSessionState::Running);
        assert!(matches!(
            fixture.snapshots.reconcile(&target, old).await,
            Err(SnapshotError::Cancelled)
        ));
        assert!(matches!(
            fixture.snapshots.caddy.observe().await.mode,
            Some(caddy::CaddyModeArtifact::Blocked { .. })
        ));
        target.home_epoch = Some(7);
        fixture.snapshots.reconcile(&target, replacement).await?;
        assert!(matches!(
            fixture.snapshots.caddy.observe().await.mode,
            Some(caddy::CaddyModeArtifact::Ready { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn observation_blocks_a_previously_verified_home_that_needs_recovery()
    -> Result<(), Box<dyn std::error::Error>> {
        let (fixture, mut target) = ready_fixture().await?;
        target.home_epoch = Some(7);
        let fence = CancellationToken::new();
        fixture.snapshots.begin_plan(&fence)?;
        fixture.snapshots.reconcile(&target, fence).await?;
        fixture
            .helper
            .lock()
            .map_err(|_| "fixture lock")?
            .home_failures = usize::MAX;
        let actual = fixture
            .snapshots
            .observe(Some(&target))
            .await?
            .actual
            .ok_or("missing actual")?;
        assert_eq!(
            actual.home.ok_or("missing Home")?.state,
            i32::from(HomeState::RecoveryRequired)
        );
        assert!(matches!(
            fixture.snapshots.caddy.observe().await.mode,
            Some(caddy::CaddyModeArtifact::Blocked { .. })
        ));
        fixture
            .helper
            .lock()
            .map_err(|_| "fixture lock")?
            .home_failures = 0;
        let fence = CancellationToken::new();
        fixture.snapshots.begin_plan(&fence)?;
        fixture.snapshots.reconcile(&target, fence).await?;
        assert!(matches!(
            fixture.snapshots.caddy.observe().await.mode,
            Some(caddy::CaddyModeArtifact::Ready { .. })
        ));
        Ok(())
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
