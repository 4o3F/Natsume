//! Guards local access and grants it only from freshly verified resource observations.

use natsume_device_protocol::generated::{
    HomeActualState, HomeState, RuntimeConfigActualState, RuntimeConfigState,
    SessionControlActualState, SessionState,
};
use tokio_util::sync::CancellationToken;

use super::{
    ReconcileOutcome, SnapshotError, SnapshotReconciler, ValidatedSnapshot,
    caddy::CaddyObservation,
    check_cancellation,
    configuration::{AppliedConfiguration, ConfigurationObservation},
    gateway::GatewayMaterialState,
    maintenance::MaintenanceOutcome,
};

pub(super) struct LocalAccessObservation {
    pub(super) session: SessionControlActualState,
    pub(super) home: HomeActualState,
    pub(super) allowed: bool,
}

pub(super) struct AccessOutcome {
    pub(super) session_actual: ReconcileOutcome<SessionControlActualState>,
    pub(super) home_actual: ReconcileOutcome<HomeActualState>,
    pub(super) caddy: CaddyObservation,
    pub(super) caddy_failed: bool,
}

impl SnapshotReconciler {
    /// Revokes Binding input and confirms the loaded data plane is blocked.
    pub(crate) async fn deactivate(&self) -> Result<bool, SnapshotError> {
        let presentation_changed = self.binding_input.revoke_eligibility()?;
        let material = self.gateway.active_material();
        self.caddy
            .ensure_blocked(material.as_ref(), &CancellationToken::new())
            .await
            .map(|_| presentation_changed)
    }

    /// Samples the initial resources and blocks stale access before configuration changes.
    pub(super) async fn inspect_and_guard(
        &self,
        snapshot: &ValidatedSnapshot,
        cancellation: &CancellationToken,
    ) -> Result<(LocalAccessObservation, ConfigurationObservation), SnapshotError> {
        check_cancellation(cancellation)?;
        let local = self.guard_local_access(Some(snapshot)).await?;
        let gateway_before = self.gateway.current_material(&snapshot.gateway_target);
        let runtime = self.runtime.observe();
        let origin_applied = applied_runtime_origin(&snapshot.runtime_origin, &runtime).is_some();
        let binding_is_applied = self.binding.is_applied(&snapshot.binding_target);
        let current_caddy = if local.allowed && origin_applied && binding_is_applied {
            self.current_caddy(snapshot, &gateway_before).await
        } else {
            None
        };
        check_cancellation(cancellation)?;
        if current_caddy.is_none() {
            self.deactivate().await?;
        }
        Ok((
            local,
            ConfigurationObservation {
                gateway_before,
                runtime,
                origin_applied,
                binding_is_applied,
            },
        ))
    }

    /// Reobserves maintenance results before applying Caddy and Binding eligibility.
    pub(super) async fn reconcile_access(
        &self,
        snapshot: &ValidatedSnapshot,
        configuration: &AppliedConfiguration,
        maintenance: MaintenanceOutcome,
        cancellation: &CancellationToken,
    ) -> Result<AccessOutcome, SnapshotError> {
        let MaintenanceOutcome {
            mut session_actual,
            mut home_actual,
            maintenance_complete,
        } = maintenance;
        let reconciled_access =
            local_access_is_allowed(snapshot, &session_actual.actual, &home_actual.actual);
        if !reconciled_access {
            self.deactivate().await?;
        }
        check_cancellation(cancellation)?;
        // Home may replace the contest generation. Sample both resources again
        // before opening access, while retaining resource failures.
        let LocalAccessObservation {
            session: session_observed,
            home: home_observed,
            allowed: observed_access,
        } = self.guard_local_access(Some(snapshot)).await?;
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
        check_cancellation(cancellation)?;
        let (caddy, caddy_failed) = self
            .load_caddy(
                snapshot,
                &configuration.gateway_material.actual,
                &configuration.runtime_actual.actual,
                allowed,
                cancellation,
            )
            .await?;
        self.binding_input.set_eligible(
            cancellation,
            allowed
                && snapshot.binding_target.bound.is_none()
                && self.session.binding_foreground_ready().await?,
        )?;
        // Selecting Binding changes the UI revision. Never publish readiness
        // copied from the preceding placeholder frame.
        session_actual.actual = self.session.observe().await?;
        session_actual.retry |= maintenance_complete && !session_actual.actual.waiting_ready;
        Ok(AccessOutcome {
            session_actual,
            home_actual,
            caddy,
            caddy_failed,
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
    pub(super) async fn guard_local_access(
        &self,
        target: Option<&ValidatedSnapshot>,
    ) -> Result<LocalAccessObservation, SnapshotError> {
        let mut session = self.session.observe().await;
        let session_allowed = match (target, &session) {
            (Some(target), Ok(session)) => session_access_is_allowed(target, session),
            _ => false,
        };
        if !session_allowed
            && self.deactivate().await?
            && let Ok(session) = &mut session
        {
            session.waiting_ready = false;
        }
        let home = self.home.observe().await;
        let allowed = match (target, &session, &home) {
            (Some(target), Ok(session), Ok(home)) => local_access_is_allowed(target, session, home),
            _ => false,
        };
        if session_allowed
            && !allowed
            && self.deactivate().await?
            && let Ok(session) = &mut session
        {
            session.waiting_ready = false;
        }
        Ok(LocalAccessObservation {
            session: session?,
            home: home?,
            allowed,
        })
    }
}

pub(super) fn local_access_is_allowed(
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

pub(super) fn applied_runtime_origin<'a>(
    target_origin: &'a str,
    actual: &'a RuntimeConfigActualState,
) -> Option<&'a str> {
    (actual.state == i32::from(RuntimeConfigState::Applied)
        && actual.applied_domjudge_origin.as_deref() == Some(target_origin))
    .then_some(target_origin)
}
