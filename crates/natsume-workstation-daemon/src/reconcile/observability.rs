use natsume_workstation_protocol::{
    generated::{
        ClientStateSnapshot, ConcreteTargetState, ForegroundTarget, HomeState, SessionForeground,
        SessionState,
    },
    target_digest,
};
use tokio::time::{Duration, Instant};

use super::{ReconcileOutcome, SnapshotError};

#[derive(Clone, Debug)]
pub(crate) struct TargetMetadata {
    pub(crate) target_digest: String,
    pub(crate) foreground_target: ForegroundTarget,
    pub(crate) terminate_epoch: Option<u64>,
    pub(crate) reset_epoch: Option<u64>,
}

impl TargetMetadata {
    pub(crate) fn new(target: &ConcreteTargetState) -> Self {
        Self {
            target_digest: target_digest(target),
            foreground_target: target
                .session_control
                .as_ref()
                .map_or(ForegroundTarget::Unspecified, |session| {
                    session.foreground_target()
                }),
            terminate_epoch: target
                .session_control
                .as_ref()
                .and_then(|session| session.terminate_epoch),
            reset_epoch: target.home.as_ref().and_then(|home| home.reset_epoch),
        }
    }

    pub(crate) fn trace_received(&self, since_previous: Option<Duration>, action: &'static str) {
        let foreground_target = match self.foreground_target {
            ForegroundTarget::Waiting => "waiting",
            ForegroundTarget::Contest => "contest",
            ForegroundTarget::Unspecified => "unspecified",
        };
        tracing::info!(target_digest = %self.target_digest, foreground_target,
            terminate_epoch = self.terminate_epoch, reset_epoch = self.reset_epoch,
            since_previous_ms = since_previous.map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
            action, "target received");
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PlanContext {
    pub(crate) plan: u64,
    pub(crate) target: TargetMetadata,
}

#[derive(Clone, Copy)]
pub(super) enum Stage {
    InspectGuard,
    Configuration,
    Maintenance,
    Access,
    Power,
    Snapshot,
}

pub(super) struct StageTimings {
    started_at: Instant,
    boundary_at: Instant,
    stage: Stage,
    /// From `inspect_and_guard` entry to configuration entry or an early result.
    inspect_guard: Option<Duration>,
    /// From configuration entry to maintenance entry or an early result.
    configuration: Option<Duration>,
    /// From maintenance entry to access entry or an early result.
    maintenance: Option<Duration>,
    /// From access entry to power entry or an early result.
    access: Option<Duration>,
    /// From power entry to snapshot entry or an early result.
    power: Option<Duration>,
    /// From the final cancellation check through snapshot construction to the result.
    snapshot: Option<Duration>,
}

impl StageTimings {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            started_at: now,
            boundary_at: now,
            stage: Stage::InspectGuard,
            inspect_guard: None,
            configuration: None,
            maintenance: None,
            access: None,
            power: None,
            snapshot: None,
        }
    }

    pub(super) fn advance(&mut self, next: Stage, now: Instant) {
        self.record_current(now);
        self.boundary_at = now;
        self.stage = next;
    }

    pub(super) fn finish(&mut self, now: Instant) -> Duration {
        self.record_current(now);
        now.duration_since(self.started_at)
    }

    fn record_current(&mut self, now: Instant) {
        let duration = match self.stage {
            Stage::InspectGuard => &mut self.inspect_guard,
            Stage::Configuration => &mut self.configuration,
            Stage::Maintenance => &mut self.maintenance,
            Stage::Access => &mut self.access,
            Stage::Power => &mut self.power,
            Stage::Snapshot => &mut self.snapshot,
        };
        *duration = Some(now.duration_since(self.boundary_at));
    }

    pub(super) fn trace_completed(
        &self,
        context: &PlanContext,
        result: &Result<ReconcileOutcome<ClientStateSnapshot>, SnapshotError>,
        elapsed: Duration,
        retry: &RetryDecisions,
    ) {
        let outcome = match result {
            Ok(outcome) if outcome.retry => "retry",
            Ok(_) => "completed",
            Err(SnapshotError::Cancelled) => "cancelled",
            Err(_) => "error",
        };
        tracing::info!(plan = context.plan, target_digest = %context.target.target_digest,
            outcome, elapsed_us = micros(elapsed), inspect_guard_us = self.inspect_guard.map(micros),
            configuration_us = self.configuration.map(micros), maintenance_us = self.maintenance.map(micros), access_us = self.access.map(micros),
            power_us = self.power.map(micros), snapshot_us = self.snapshot.map(micros),
            gateway_retry = retry.gateway, runtime_retry = retry.runtime, session_retry = retry.session,
            home_retry = retry.home, power_retry = retry.power, caddy_retry = retry.caddy,
            "reconcile completed");
    }
}

#[derive(Default)]
pub(super) struct RetryDecisions {
    pub(super) gateway: Option<bool>,
    pub(super) runtime: Option<bool>,
    pub(super) session: Option<bool>,
    pub(super) home: Option<bool>,
    pub(super) power: Option<bool>,
    pub(super) caddy: Option<bool>,
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

pub(crate) enum SnapshotSource<'a> {
    Plan(&'a PlanContext),
    Observation(Option<&'a TargetMetadata>),
}

impl SnapshotSource<'_> {
    pub(crate) fn trace_sent(&self, snapshot: &ClientStateSnapshot) {
        let (source, plan, target) = match self {
            Self::Plan(context) => ("plan", Some(context.plan), Some(&context.target)),
            Self::Observation(target) => ("observation", None, *target),
        };
        let session = snapshot
            .actual
            .as_ref()
            .and_then(|actual| actual.session_control.as_ref());
        let home = snapshot
            .actual
            .as_ref()
            .and_then(|actual| actual.home.as_ref());
        let session_foreground = session.map(|session| match session.foreground() {
            SessionForeground::Unknown => "unknown",
            SessionForeground::None => "none",
            SessionForeground::Waiting => "waiting",
            SessionForeground::Contest => "contest",
            SessionForeground::Greeter => "greeter",
            SessionForeground::Other => "other",
        });
        let session_state = session.map(|session| match session.session_state() {
            SessionState::Unspecified => "unspecified",
            SessionState::None => "none",
            SessionState::Starting => "starting",
            SessionState::Running => "running",
            SessionState::Ambiguous => "ambiguous",
            SessionState::Terminating => "terminating",
            SessionState::Error => "error",
        });
        let home_state = home.map(|home| match home.state() {
            HomeState::Unspecified => "unspecified",
            HomeState::Steady => "steady",
            HomeState::Resetting => "resetting",
            HomeState::RecoveryRequired => "recovery_required",
        });
        tracing::info!(
            source,
            plan,
            target_digest = target.map(|target| target.target_digest.as_str()),
            session_foreground,
            session_state,
            home_state,
            waiting_ready = session.map(|session| session.waiting_ready),
            contest_ready = session.map(|session| session.contest_ready),
            completed_terminate_epoch =
                session.and_then(|session| session.completed_terminate_epoch),
            completed_reset_epoch = home.and_then(|home| home.completed_reset_epoch),
            "actual sent"
        );
    }
}

#[cfg(test)]
pub(crate) mod tests;
