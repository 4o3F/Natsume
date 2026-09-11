use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use natsume_device_protocol::generated::{
    ForegroundTarget, SessionControlActualState, SessionControlTarget,
    SessionForeground as WireForeground, SessionState,
};
use natsume_local_control_api::{
    GraphicalSession, GraphicalSessionState, ManagedSessionsObservation, Privileged1Proxy,
    SessionForeground, SessionRole,
};
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::atomic_write::{WritePolicy, atomic_write};

use super::{
    ReconcileOutcome, SnapshotError, binding::BindingInputProvider, check_cancellation,
    invalid_epoch,
};

const ARTIFACT_FORMAT_VERSION: u32 = 1;
const WAITING_FAILURE_COOLDOWN: Duration = Duration::from_mins(1);
const MAX_WAITING_SAMPLE_GAP: Duration = Duration::from_secs(45);

#[derive(Default)]
struct WaitingHealth {
    failed_since: Option<Instant>,
    last_sample: Option<Instant>,
    attempted: bool,
}

impl WaitingHealth {
    fn observe(&mut self, now: Instant, ready: bool) -> bool {
        if ready {
            *self = Self::default();
            return false;
        }
        if self
            .last_sample
            .is_none_or(|last| now.duration_since(last) > MAX_WAITING_SAMPLE_GAP)
        {
            self.failed_since = Some(now);
        }
        self.last_sample = Some(now);
        !self.attempted
            && self
                .failed_since
                .is_some_and(|since| now.duration_since(since) >= WAITING_FAILURE_COOLDOWN)
    }
}

/// Durable terminate progress which fences one transition to one exact graphical session.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionCompletionArtifact {
    format_version: u32,
    completed_terminate_epoch: Option<u64>,
    pending: Option<PendingTermination>,
}

/// Exact session captured before applying one terminate transition.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PendingTermination {
    terminate_epoch: u64,
    session: GraphicalSession,
}

/// Session target parsed at the complete Server snapshot boundary.
#[derive(PartialEq)]
pub(super) struct ValidatedSessionTarget {
    foreground_target: SessionRole,
    terminate_epoch: Option<u64>,
}

impl ValidatedSessionTarget {
    pub(super) fn termination_is_complete(&self, actual: &SessionControlActualState) -> bool {
        actual.completed_terminate_epoch == self.terminate_epoch
    }

    pub(super) fn has_new_termination(&self, actual: &SessionControlActualState) -> bool {
        self.terminate_epoch.is_some_and(|epoch| {
            actual
                .completed_terminate_epoch
                .is_none_or(|completed| epoch > completed)
        })
    }
}

pub(super) fn validate_target(target: SessionControlTarget) -> Option<ValidatedSessionTarget> {
    let foreground_target = match ForegroundTarget::try_from(target.foreground_target).ok()? {
        ForegroundTarget::Contest => SessionRole::Contest,
        ForegroundTarget::Waiting => SessionRole::Waiting,
        ForegroundTarget::Unspecified => return None,
    };
    if target.terminate_epoch.is_some_and(invalid_epoch) {
        return None;
    }
    Some(ValidatedSessionTarget {
        foreground_target,
        terminate_epoch: target.terminate_epoch,
    })
}

/// Session Control reconciler using the helper's exact-session capabilities.
pub(super) struct SessionReconciler {
    connection: zbus::Connection,
    artifact_path: PathBuf,
    agent: Arc<BindingInputProvider>,
    waiting_health: Mutex<WaitingHealth>,
}

impl SessionReconciler {
    pub(super) fn production(
        connection: zbus::Connection,
        agent: Arc<BindingInputProvider>,
    ) -> Self {
        Self {
            connection,
            agent,
            artifact_path: PathBuf::from("/var/lib/natsume/state/session-completion.json"),
            waiting_health: Mutex::new(WaitingHealth::default()),
        }
    }

    async fn observation_actual(
        &self,
        observed: &ManagedSessionsObservation,
        completed: Option<u64>,
    ) -> SessionControlActualState {
        let mut actual = observation_actual(observed, completed);
        actual.waiting_ready = observed.waiting.state == GraphicalSessionState::Running
            && observed.waiting.desktop_ready
            && !observed.waiting.locked_hint
            && if let Some(session) = observed.waiting.session.as_ref() {
                self.agent.waiting_ready(&self.connection, session).await
            } else {
                false
            };
        // Reconciliation also publishes healthy observations between the idle
        // maintenance ticks. A recovered display ends the continuous-failure
        // interval before another fault can start.
        if actual.waiting_ready
            && let Ok(mut health) = self.waiting_health.lock()
        {
            health.observe(Instant::now(), true);
        }
        actual
    }

    async fn proxy(&self) -> Result<Privileged1Proxy<'_>, SnapshotError> {
        Privileged1Proxy::new(&self.connection)
            .await
            .map_err(|_| SnapshotError::LocalControl)
    }

    /// Local recovery is independent of business Targets. The current control
    /// task tracks this bounded call; only Helper can spend the per-boot budget.
    pub(super) async fn maintain_waiting(&self) -> Result<(), SnapshotError> {
        let proxy = self.proxy().await?;
        let Ok(observed) = proxy.query_managed_sessions().await else {
            let mut health = self
                .waiting_health
                .lock()
                .map_err(|_| SnapshotError::Artifact)?;
            health.failed_since = None;
            health.last_sample = None;
            return Err(SnapshotError::LocalControl);
        };
        let ready = self.observation_actual(&observed, None).await.waiting_ready;
        let due = self
            .waiting_health
            .lock()
            .map_err(|_| SnapshotError::Artifact)?
            .observe(Instant::now(), ready);
        if !ready {
            self.agent.revoke_eligibility()?;
        }
        // An already captured operation is resumed without another cooldown,
        // including after a Daemon/Helper restart. It never captures new work.
        match proxy.resume_waiting_recovery().await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, "Owned waiting recovery remains incomplete");
                return Ok(());
            }
        }
        if due {
            self.agent
                .retire_waiting(observed.waiting.session.as_ref())?;
            match proxy
                .recover_waiting_session(&observed.waiting.session)
                .await
            {
                Ok(_) => {
                    self.waiting_health
                        .lock()
                        .map_err(|_| SnapshotError::Artifact)?
                        .attempted = true;
                }
                Err(error) => tracing::warn!(%error, "Waiting recovery could not complete"),
            }
        }
        Ok(())
    }

    pub(super) async fn reconcile(
        &self,
        target: &ValidatedSessionTarget,
        waiting: Option<&GraphicalSession>,
        cancellation: &CancellationToken,
    ) -> Result<ReconcileOutcome<SessionControlActualState>, SnapshotError> {
        let mut completion = match read_completion(&self.artifact_path) {
            CompletionState::Valid(completion) => completion,
            CompletionState::Absent => SessionCompletionArtifact {
                format_version: ARTIFACT_FORMAT_VERSION,
                completed_terminate_epoch: None,
                pending: None,
            },
            CompletionState::Failed => return Ok(ReconcileOutcome::idle(error_actual(None))),
        };
        let proxy = self.proxy().await?;

        if let Some(actual) = self.resume_pending(&proxy, &mut completion).await? {
            return Ok(actual);
        }
        if let Some(actual) = self
            .apply_terminate_epoch(
                &proxy,
                &mut completion,
                target.terminate_epoch,
                waiting,
                cancellation,
            )
            .await?
        {
            return Ok(actual);
        }
        check_cancellation(cancellation)?;
        let observed = match proxy.query_managed_sessions().await {
            Ok(observation) => {
                let actual = self
                    .observation_actual(&observation, completion.completed_terminate_epoch)
                    .await;
                ReconcileOutcome::idle(actual)
            }
            Err(error) => ReconcileOutcome::control_error(
                error_actual(completion.completed_terminate_epoch),
                &error,
            ),
        };
        Ok(observed)
    }

    async fn resume_pending(
        &self,
        proxy: &Privileged1Proxy<'_>,
        completion: &mut SessionCompletionArtifact,
    ) -> Result<Option<ReconcileOutcome<SessionControlActualState>>, SnapshotError> {
        let Some(pending) = completion.pending.as_ref() else {
            return Ok(None);
        };
        // This is already-owned work on one exact boot/session. Revoking a
        // Target cannot revoke a logind call or make a replacement its target.
        if let Err(error) = proxy.terminate_contest_session(&pending.session).await {
            return Ok(Some(ReconcileOutcome::control_error(
                terminating_actual(completion.completed_terminate_epoch),
                &error,
            )));
        }
        completion.completed_terminate_epoch = Some(pending.terminate_epoch);
        completion.pending = None;
        persist_completion(&self.artifact_path, completion)?;
        Ok(None)
    }

    async fn apply_terminate_epoch(
        &self,
        proxy: &Privileged1Proxy<'_>,
        completion: &mut SessionCompletionArtifact,
        target_epoch: Option<u64>,
        waiting: Option<&GraphicalSession>,
        cancellation: &CancellationToken,
    ) -> Result<Option<ReconcileOutcome<SessionControlActualState>>, SnapshotError> {
        let Some(epoch) = target_epoch.filter(|epoch| {
            completion
                .completed_terminate_epoch
                .is_none_or(|completed| completed < *epoch)
        }) else {
            return Ok(None);
        };
        check_cancellation(cancellation)?;
        if waiting.is_none() {
            return Ok(Some(ReconcileOutcome::retry(self.observe().await?)));
        }
        let observation = match proxy.query_managed_sessions().await {
            Ok(observation) => observation,
            Err(error) => {
                return Ok(Some(ReconcileOutcome::control_error(
                    error_actual(completion.completed_terminate_epoch),
                    &error,
                )));
            }
        };
        if observation.waiting.session.as_ref() != waiting
            || observation.foreground != SessionForeground::Waiting
            || !self
                .observation_actual(&observation, completion.completed_terminate_epoch)
                .await
                .waiting_ready
        {
            return Ok(Some(ReconcileOutcome::retry(self.observe().await?)));
        }
        match observation.contest.state {
            GraphicalSessionState::Ambiguous | GraphicalSessionState::Error => {
                Ok(Some(ReconcileOutcome::idle(
                    self.observation_actual(&observation, completion.completed_terminate_epoch)
                        .await,
                )))
            }
            GraphicalSessionState::None => {
                check_cancellation(cancellation)?;
                completion.completed_terminate_epoch = Some(epoch);
                persist_completion(&self.artifact_path, completion)?;
                Ok(None)
            }
            GraphicalSessionState::Running
            | GraphicalSessionState::Starting
            | GraphicalSessionState::Terminating => {
                let Some(session) = observation.contest.session.as_ref() else {
                    return Ok(Some(ReconcileOutcome::idle(error_actual(
                        completion.completed_terminate_epoch,
                    ))));
                };
                check_cancellation(cancellation)?;
                completion.pending = Some(PendingTermination {
                    terminate_epoch: epoch,
                    session: session.clone(),
                });
                persist_completion(&self.artifact_path, completion)?;
                check_cancellation(cancellation)?;
                if let Err(error) = proxy.terminate_contest_session(session).await {
                    return Ok(Some(ReconcileOutcome::control_error(
                        terminating_actual(completion.completed_terminate_epoch),
                        &error,
                    )));
                }
                completion.completed_terminate_epoch = Some(epoch);
                completion.pending = None;
                persist_completion(&self.artifact_path, completion)?;
                Ok(None)
            }
        }
    }

    pub(super) async fn observe(&self) -> Result<SessionControlActualState, SnapshotError> {
        let (completion, pending) = match read_completion(&self.artifact_path) {
            CompletionState::Absent => (None, false),
            CompletionState::Valid(completion) => (
                completion.completed_terminate_epoch,
                completion.pending.is_some(),
            ),
            CompletionState::Failed => return Ok(error_actual(None)),
        };
        let observation = match self.proxy().await?.query_managed_sessions().await {
            Ok(observation) => observation,
            Err(_) if pending => return Ok(terminating_actual(completion)),
            Err(_) => return Err(SnapshotError::LocalControl),
        };
        let mut actual = self.observation_actual(&observation, completion).await;
        if pending {
            actual.session_state = SessionState::Terminating.into();
            actual.contest_ready = false;
        }
        Ok(actual)
    }

    /// Supplies a maintenance prerequisite only after checking both the current
    /// Agent frame and the exact OS foreground, including after activation.
    pub(super) async fn waiting_foreground(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<ReconcileOutcome<Option<GraphicalSession>>, SnapshotError> {
        let proxy = self.proxy().await?;
        let observed = proxy
            .query_managed_sessions()
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        let actual = self.observation_actual(&observed, None).await;
        let Some(waiting) = observed.waiting.session.filter(|_| actual.waiting_ready) else {
            return Ok(ReconcileOutcome::retry(None));
        };
        check_cancellation(cancellation)?;
        if observed.foreground != SessionForeground::Waiting
            && let Err(error) = proxy.activate_session(SessionRole::Waiting, &waiting).await
        {
            return Ok(ReconcileOutcome::control_error(None, &error));
        }
        let after = proxy
            .query_managed_sessions()
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        let actual = self.observation_actual(&after, None).await;
        check_cancellation(cancellation)?;
        Ok(
            if after.waiting.session.as_ref() == Some(&waiting)
                && foreground_is_ready(SessionRole::Waiting, &actual)
            {
                ReconcileOutcome::idle(Some(waiting))
            } else {
                ReconcileOutcome::retry(None)
            },
        )
    }

    /// The root-owned boot fence is shared by offline startup and online plans.
    /// It is idempotent after this boot's initial return to waiting.
    pub(super) async fn prepare_boot(
        &self,
        cancellation: Option<&CancellationToken>,
    ) -> Result<bool, SnapshotError> {
        let proxy = self.proxy().await?;
        let observed = proxy
            .query_managed_sessions()
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        let actual = self.observation_actual(&observed, None).await;
        let Some(waiting) = observed.waiting.session.filter(|_| actual.waiting_ready) else {
            return Ok(false);
        };
        if let Some(cancellation) = cancellation {
            check_cancellation(cancellation)?;
        }
        proxy
            .prepare_boot_sessions(&waiting)
            .await
            .map_err(|_| SnapshotError::LocalControl)
    }

    pub(super) async fn binding_foreground_ready(&self) -> Result<bool, SnapshotError> {
        let observed = self
            .proxy()
            .await?
            .query_managed_sessions()
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        Ok(observed.foreground == SessionForeground::Waiting
            && observed.waiting.state == GraphicalSessionState::Running
            && observed.waiting.desktop_ready
            && !observed.waiting.locked_hint
            && if let Some(waiting) = observed.waiting.session.as_ref() {
                self.agent
                    .waiting_agent_alive(&self.connection, waiting)
                    .await
            } else {
                false
            })
    }

    /// Prepares desktops only after maintenance completion, then applies the
    /// effective foreground of this still-current plan.
    pub(super) async fn present(
        &self,
        target: &ValidatedSessionTarget,
        bound: bool,
        maintenance_complete: bool,
        cancellation: &CancellationToken,
    ) -> Result<ReconcileOutcome<SessionControlActualState>, SnapshotError> {
        check_cancellation(cancellation)?;
        if !maintenance_complete {
            let waiting = self.waiting_foreground(cancellation).await?;
            return Ok(ReconcileOutcome {
                actual: self.observe().await?,
                retry: waiting.retry,
            });
        }
        if !self.prepare_boot(Some(cancellation)).await? {
            return Ok(ReconcileOutcome::retry(self.observe().await?));
        }
        check_cancellation(cancellation)?;
        let proxy = self.proxy().await?;
        let observed = proxy
            .query_managed_sessions()
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        // A failed preparation unit reports Error without a login identity.
        // Helper rechecks the actual role and admission before retrying it.
        if observed.contest.session.is_none()
            && matches!(
                observed.contest.state,
                GraphicalSessionState::None | GraphicalSessionState::Error
            )
        {
            check_cancellation(cancellation)?;
            if let Err(error) = proxy.prepare_session(SessionRole::Contest).await {
                return Ok(ReconcileOutcome::control_error(
                    self.observe().await?,
                    &error,
                ));
            }
        }
        let observed = proxy
            .query_managed_sessions()
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        let actual = self.observation_actual(&observed, None).await;
        let effective = if bound {
            target.foreground_target
        } else {
            SessionRole::Waiting
        };
        let (session, ready, foreground) = match effective {
            SessionRole::Waiting => (
                observed.waiting.session,
                actual.waiting_ready,
                SessionForeground::Waiting,
            ),
            SessionRole::Contest => (
                observed.contest.session,
                actual.contest_ready,
                SessionForeground::Contest,
            ),
        };
        if let Some(session) = session.filter(|_| ready)
            && observed.foreground != foreground
        {
            check_cancellation(cancellation)?;
            if let Err(error) = proxy.activate_session(effective, &session).await {
                return Ok(ReconcileOutcome::control_error(
                    self.observe().await?,
                    &error,
                ));
            }
        }
        check_cancellation(cancellation)?;
        let actual = self.observe().await?;
        Ok(ReconcileOutcome {
            retry: !foreground_is_ready(effective, &actual) || !actual.contest_ready,
            actual,
        })
    }

    /// Completes only durable pending termination, without capturing a new target.
    pub(super) async fn recover_owned(
        &self,
    ) -> Result<ReconcileOutcome<SessionControlActualState>, SnapshotError> {
        if let CompletionState::Valid(mut completion) = read_completion(&self.artifact_path) {
            let proxy = self.proxy().await?;
            if let Some(outcome) = self.resume_pending(&proxy, &mut completion).await? {
                return Ok(outcome);
            }
        }
        self.observe().await.map(ReconcileOutcome::idle)
    }
}

fn foreground_is_ready(role: SessionRole, actual: &SessionControlActualState) -> bool {
    match role {
        SessionRole::Waiting => {
            actual.foreground == i32::from(WireForeground::Waiting) && actual.waiting_ready
        }
        SessionRole::Contest => {
            actual.foreground == i32::from(WireForeground::Contest)
                && actual.session_state == i32::from(SessionState::Running)
                && actual.contest_ready
        }
    }
}

enum CompletionState {
    Absent,
    Valid(SessionCompletionArtifact),
    Failed,
}

fn read_completion(path: &std::path::Path) -> CompletionState {
    let encoded = match fs::read(path) {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CompletionState::Absent;
        }
        Err(_) => return CompletionState::Failed,
    };
    let Ok(artifact) = serde_json::from_slice::<SessionCompletionArtifact>(&encoded) else {
        return CompletionState::Failed;
    };
    if artifact.format_version != ARTIFACT_FORMAT_VERSION
        || artifact
            .completed_terminate_epoch
            .is_some_and(invalid_epoch)
        || artifact.pending.as_ref().is_some_and(|pending| {
            invalid_epoch(pending.terminate_epoch)
                || artifact
                    .completed_terminate_epoch
                    .is_some_and(|completed| completed >= pending.terminate_epoch)
                || pending.session.logind_session_id.is_empty()
                || !valid_boot_id(&pending.session.boot_id)
        })
    {
        return CompletionState::Failed;
    }
    CompletionState::Valid(artifact)
}

fn persist_completion(
    path: &std::path::Path,
    artifact: &SessionCompletionArtifact,
) -> Result<(), SnapshotError> {
    let encoded = serde_json::to_vec(artifact).map_err(|_| SnapshotError::Artifact)?;
    atomic_write(path, &encoded, 0o600, WritePolicy::Replace).map_err(|_| SnapshotError::Artifact)
}

fn valid_boot_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|parsed| parsed.hyphenated().to_string() == value)
}

fn observation_actual(
    observation: &ManagedSessionsObservation,
    completed_terminate_epoch: Option<u64>,
) -> SessionControlActualState {
    let state = match observation.contest.state {
        GraphicalSessionState::None => SessionState::None,
        GraphicalSessionState::Starting => SessionState::Starting,
        GraphicalSessionState::Running => SessionState::Running,
        GraphicalSessionState::Terminating => SessionState::Terminating,
        GraphicalSessionState::Ambiguous => SessionState::Ambiguous,
        GraphicalSessionState::Error => SessionState::Error,
    };
    let foreground = match observation.foreground {
        SessionForeground::Unknown => WireForeground::Unknown,
        SessionForeground::Waiting => WireForeground::Waiting,
        SessionForeground::Contest => WireForeground::Contest,
        SessionForeground::Greeter => WireForeground::Greeter,
        SessionForeground::Other => WireForeground::Other,
        SessionForeground::None => WireForeground::None,
    };
    SessionControlActualState {
        session_state: state.into(),
        completed_terminate_epoch,
        foreground: foreground.into(),
        // Helper desktop facts alone cannot confirm an Agent frame.
        waiting_ready: false,
        contest_ready: state == SessionState::Running
            && observation.contest.session.is_some()
            && observation.contest.desktop_ready
            && !observation.contest.locked_hint,
    }
}

fn terminating_actual(completed_terminate_epoch: Option<u64>) -> SessionControlActualState {
    SessionControlActualState {
        session_state: SessionState::Terminating.into(),
        completed_terminate_epoch,
        ..SessionControlActualState::default()
    }
}

fn error_actual(completed_terminate_epoch: Option<u64>) -> SessionControlActualState {
    SessionControlActualState {
        session_state: SessionState::Error.into(),
        completed_terminate_epoch,
        ..SessionControlActualState::default()
    }
}

#[cfg(test)]
pub(super) mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn waiting_recovery_requires_sixty_seconds_of_observed_failure() {
        let start = Instant::now();
        let mut health = WaitingHealth::default();
        for seconds in [0, 20, 40, 59] {
            assert!(!health.observe(start + Duration::from_secs(seconds), false));
        }
        assert!(health.observe(start + Duration::from_mins(1), false));
        health.attempted = true;
        assert!(!health.observe(start + Duration::from_secs(80), false));
        assert!(!health.observe(start + Duration::from_secs(100), true));
        for seconds in [101, 121, 141, 160] {
            assert!(!health.observe(start + Duration::from_secs(seconds), false));
        }
        // This can request recovery again, but Helper retains its spent budget.
        assert!(health.observe(start + Duration::from_secs(161), false));
    }

    #[test]
    fn an_unobserved_interval_does_not_complete_the_waiting_cooldown() {
        let start = Instant::now();
        let mut health = WaitingHealth::default();
        assert!(!health.observe(start, false));
        assert!(!health.observe(start + Duration::from_secs(20), false));
        assert!(!health.observe(start + Duration::from_secs(70), false));
        assert!(!health.observe(start + Duration::from_secs(100), false));
        assert!(health.observe(start + Duration::from_secs(130), false));
    }

    #[tokio::test]
    async fn a_healthy_session_observation_ends_the_previous_recovery_interval()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture =
            crate::reconcile::tests::fixture(crate::reconcile::tests::HelperState::default())
                .await?;
        let session = &fixture.snapshots.session;
        {
            let now = Instant::now();
            let mut health = session.waiting_health.lock().map_err(|_| "health lock")?;
            health.failed_since = Some(now - WAITING_FAILURE_COOLDOWN);
            health.last_sample = Some(now - Duration::from_secs(30));
        }
        // This is the observation used by ordinary reconciliation, without a
        // maintenance tick in between recovery and the next display failure.
        assert!(session.observe().await?.waiting_ready);
        let mut health = session.waiting_health.lock().map_err(|_| "health lock")?;
        assert!(!health.observe(Instant::now(), false));
        assert!(
            fixture
                .helper
                .lock()
                .map_err(|_| "helper lock")?
                .waiting_recovery_calls
                .is_empty()
        );
        Ok(())
    }

    pub(in crate::reconcile) fn reconciler(
        directory: &TempDir,
        connection: zbus::Connection,
        agent: Arc<BindingInputProvider>,
    ) -> SessionReconciler {
        SessionReconciler {
            connection,
            artifact_path: directory.path().join("session-completion.json"),
            agent,
            waiting_health: Mutex::new(WaitingHealth::default()),
        }
    }

    #[test]
    fn pending_termination_is_durable_and_keeps_previous_completion_visible() {
        let directory = TempDir::new()
            .unwrap_or_else(|error| panic!("test directory must be created: {error}"));
        let path = directory.path().join("session-completion.json");
        let artifact = SessionCompletionArtifact {
            format_version: ARTIFACT_FORMAT_VERSION,
            completed_terminate_epoch: Some(3),
            pending: Some(PendingTermination {
                terminate_epoch: 4,
                session: GraphicalSession {
                    logind_session_id: "c2".to_owned(),
                    boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
                },
            }),
        };

        persist_completion(&path, &artifact)
            .unwrap_or_else(|error| panic!("completion must persist: {error}"));
        let CompletionState::Valid(reloaded) = read_completion(&path) else {
            panic!("completion must reload");
        };

        assert_eq!(reloaded.completed_terminate_epoch, Some(3));
        assert_eq!(
            reloaded.pending.map(|pending| pending.terminate_epoch),
            Some(4)
        );
    }

    #[test]
    fn corrupt_completion_fails_closed() {
        let directory = TempDir::new()
            .unwrap_or_else(|error| panic!("test directory must be created: {error}"));
        let path = directory.path().join("session-completion.json");
        fs::write(&path, b"not-json")
            .unwrap_or_else(|error| panic!("fixture must be written: {error}"));

        assert!(matches!(read_completion(&path), CompletionState::Failed));
    }

    #[test]
    fn pending_termination_requires_a_canonical_boot_id() {
        let directory = TempDir::new()
            .unwrap_or_else(|error| panic!("test directory must be created: {error}"));
        let path = directory.path().join("session-completion.json");
        let artifact = SessionCompletionArtifact {
            format_version: ARTIFACT_FORMAT_VERSION,
            completed_terminate_epoch: None,
            pending: Some(PendingTermination {
                terminate_epoch: 1,
                session: GraphicalSession {
                    logind_session_id: "c2".to_owned(),
                    boot_id: "not-a-boot-id".to_owned(),
                },
            }),
        };
        persist_completion(&path, &artifact)
            .unwrap_or_else(|error| panic!("fixture must persist: {error}"));

        assert!(matches!(read_completion(&path), CompletionState::Failed));
    }
    #[test]
    fn targets_accept_only_waiting_contest_and_valid_transition_epochs() {
        for (wire, role) in [
            (ForegroundTarget::Waiting, SessionRole::Waiting),
            (ForegroundTarget::Contest, SessionRole::Contest),
        ] {
            let target = validate_target(SessionControlTarget {
                foreground_target: wire.into(),
                terminate_epoch: Some(1),
            })
            .unwrap_or_else(|| panic!("managed role"));
            assert_eq!(target.foreground_target, role);
            assert_eq!(target.terminate_epoch, Some(1));
        }
        for foreground in [0, 3, 4, 5, 99, -1] {
            assert!(
                validate_target(SessionControlTarget {
                    foreground_target: foreground,
                    terminate_epoch: None
                })
                .is_none()
            );
        }
        for epoch in [0, u64::MAX] {
            assert!(
                validate_target(SessionControlTarget {
                    foreground_target: ForegroundTarget::Waiting.into(),
                    terminate_epoch: Some(epoch)
                })
                .is_none()
            );
        }
    }

    #[test]
    fn desktop_observations_do_not_substitute_for_waiting_frame_confirmation() {
        use natsume_local_control_api::GraphicalSessionObservation;
        let role = |id: &str| GraphicalSessionObservation {
            state: GraphicalSessionState::Running,
            session: Some(GraphicalSession {
                logind_session_id: id.to_owned(),
                boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
            }),
            desktop_ready: true,
            locked_hint: false,
        };
        let mut observed = ManagedSessionsObservation {
            waiting: role("w1"),
            contest: role("c2"),
            foreground: SessionForeground::Waiting,
        };
        let actual = observation_actual(&observed, Some(3));
        assert_eq!(actual.session_state, i32::from(SessionState::Running));
        assert_eq!(actual.foreground, i32::from(WireForeground::Waiting));
        assert_eq!(actual.completed_terminate_epoch, Some(3));
        assert!(actual.contest_ready);
        assert!(!actual.waiting_ready);
        assert!(!foreground_is_ready(SessionRole::Waiting, &actual));
        assert!(!foreground_is_ready(SessionRole::Contest, &actual));
        observed.foreground = SessionForeground::Contest;
        observed.contest.locked_hint = true;
        let locked = observation_actual(&observed, Some(3));
        assert_eq!(locked.session_state, i32::from(SessionState::Running));
        assert!(!locked.contest_ready);
        assert!(!locked.waiting_ready);
        assert!(!foreground_is_ready(SessionRole::Contest, &locked));
    }
}
