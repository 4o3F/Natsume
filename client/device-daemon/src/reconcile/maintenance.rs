//! Coordinates Session and Home maintenance before selecting the foreground presentation.

use natsume_device_protocol::generated::{
    ClientStateSnapshot, HomeActualState, HomeState, SessionControlActualState, SessionForeground,
    SessionState,
};
use tokio_util::sync::CancellationToken;

use super::{
    ReconcileOutcome, SnapshotError, SnapshotReconciler, ValidatedSnapshot,
    access::LocalAccessObservation, check_cancellation, local_access_is_allowed,
};

pub(super) struct MaintenanceOutcome {
    pub(super) session_actual: ReconcileOutcome<SessionControlActualState>,
    pub(super) home_actual: ReconcileOutcome<HomeActualState>,
    pub(super) maintenance_complete: bool,
}

impl SnapshotReconciler {
    /// Finishes durable local maintenance and the fixed first-boot preparation
    /// while disconnected, without new epochs or business presentation.
    pub(crate) async fn recover_local(&self) -> Result<ClientStateSnapshot, SnapshotError> {
        self.deactivate().await.map_err(|_| SnapshotError::Caddy)?;
        let session = self.session.recover_owned().await;
        // Home owns a separate durable window. An unavailable session observation
        // must not stop the attempt to recover that already-captured window.
        let home = self
            .home
            .reconcile(None, None, &CancellationToken::new())
            .await;
        let session = session?;
        let home = home?;
        if !session.retry
            && home.actual.state == i32::from(HomeState::Steady)
            && session.actual.session_state != i32::from(SessionState::Terminating)
            && session.actual.session_state != i32::from(SessionState::Error)
        {
            let boot_prepared = self.session.prepare_boot(None).await?;
            if boot_prepared && home.actual.completed_reset_epoch.is_some() {
                // GDM may select its greeter while the captured contest is
                // drained. The boot fence is already complete on a later reset;
                // return to waiting once Home is safe and no login is pending.
                // Preserve existing contest and administrator sessions offline.
                let observed = self.session.observe().await?;
                if observed.session_state == i32::from(SessionState::None)
                    && observed.foreground == i32::from(SessionForeground::Greeter)
                {
                    self.session
                        .waiting_foreground(&CancellationToken::new())
                        .await?;
                }
            }
        }
        self.observe(None).await
    }

    /// Finishes termination before Home reset, then presents the requested foreground.
    pub(super) async fn reconcile_maintenance(
        &self,
        snapshot: &ValidatedSnapshot,
        before: LocalAccessObservation,
        cancellation: &CancellationToken,
    ) -> Result<MaintenanceOutcome, SnapshotError> {
        let LocalAccessObservation {
            session: session_before,
            home: home_before,
            allowed: access_before,
        } = before;
        check_cancellation(cancellation)?;
        let needs_maintenance = snapshot.session_target.has_new_termination(&session_before)
            || snapshot.home_epoch.is_some_and(|epoch| {
                home_before
                    .completed_reset_epoch
                    .is_none_or(|completed| epoch > completed)
            });
        let waiting = if needs_maintenance {
            self.deactivate().await?;
            self.session.waiting_foreground(cancellation).await?
        } else {
            ReconcileOutcome::idle(None)
        };
        let mut session_actual = self
            .session
            .reconcile(
                &snapshot.session_target,
                waiting.actual.as_ref(),
                cancellation,
            )
            .await?;
        if access_before && !local_access_is_allowed(snapshot, &session_actual.actual, &home_before)
        {
            self.deactivate().await?;
        }
        check_cancellation(cancellation)?;
        let waiting = if snapshot.home_epoch.is_some_and(|epoch| {
            home_before
                .completed_reset_epoch
                .is_none_or(|completed| epoch > completed)
        }) {
            self.session.waiting_foreground(cancellation).await?
        } else {
            waiting
        };
        let home_actual = self
            .home
            .reconcile(snapshot.home_epoch, waiting.actual.as_ref(), cancellation)
            .await?;
        let maintenance_complete = home_actual.actual.state == i32::from(HomeState::Steady)
            && home_actual.actual.completed_reset_epoch == snapshot.home_epoch
            && snapshot
                .session_target
                .termination_is_complete(&session_actual.actual);
        let presented = self
            .session
            .present(
                &snapshot.session_target,
                snapshot.binding_target.bound.is_some(),
                maintenance_complete,
                cancellation,
            )
            .await?;
        session_actual.actual = presented.actual;
        session_actual.retry |= presented.retry || waiting.retry;
        Ok(MaintenanceOutcome {
            session_actual,
            home_actual,
            maintenance_complete,
        })
    }
}
