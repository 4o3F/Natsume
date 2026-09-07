use std::{fs, path::PathBuf};

use natsume_device_protocol::generated::{
    ForegroundTarget, SessionControlActualState, SessionControlTarget,
    SessionForeground as WireForeground, SessionState,
};
use natsume_local_control_api::{
    GraphicalSession, GraphicalSessionState, ManagedSessionsObservation, Privileged1Proxy,
    SessionForeground, SessionRole,
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::atomic_write::{WritePolicy, atomic_write};

use super::{ReconcileOutcome, SnapshotError, check_cancellation, invalid_epoch};

const ARTIFACT_FORMAT_VERSION: u32 = 1;

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
}

impl SessionReconciler {
    pub(super) fn production(connection: zbus::Connection) -> Self {
        Self {
            connection,
            artifact_path: PathBuf::from("/var/lib/natsume/state/session-completion.json"),
        }
    }

    async fn proxy(&self) -> Result<Privileged1Proxy<'_>, SnapshotError> {
        Privileged1Proxy::new(&self.connection)
            .await
            .map_err(|_| SnapshotError::LocalControl)
    }

    pub(super) async fn reconcile(
        &self,
        target: &ValidatedSessionTarget,
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

        if let Some(actual) = self
            .resume_pending(
                &proxy,
                &mut completion,
                target.terminate_epoch,
                cancellation,
            )
            .await?
        {
            return Ok(actual);
        }
        if let Some(actual) = self
            .apply_terminate_epoch(
                &proxy,
                &mut completion,
                target.terminate_epoch,
                cancellation,
            )
            .await?
        {
            return Ok(actual);
        }
        check_cancellation(cancellation)?;
        // TODO(R1/R4): activate the exact role after the Home and Binding guards.
        // Foreground targets must never be implemented through desktop Lock/Unlock.
        let observed = match proxy.query_managed_sessions().await {
            Ok(observation) => {
                let actual = observation_actual(&observation, completion.completed_terminate_epoch);
                ReconcileOutcome {
                    retry: !foreground_is_ready(target.foreground_target, &actual),
                    actual,
                }
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
        target_epoch: Option<u64>,
        cancellation: &CancellationToken,
    ) -> Result<Option<ReconcileOutcome<SessionControlActualState>>, SnapshotError> {
        let Some(pending) = completion.pending.as_ref() else {
            return Ok(None);
        };
        if !may_resume_pending(pending, target_epoch) {
            return Ok(Some(ReconcileOutcome::idle(error_actual(
                completion.completed_terminate_epoch,
            ))));
        }
        check_cancellation(cancellation)?;
        if let Err(error) = proxy.terminate_contest_session(&pending.session).await {
            return Ok(Some(ReconcileOutcome::control_error(
                terminating_actual(completion.completed_terminate_epoch),
                &error,
            )));
        }
        check_cancellation(cancellation)?;
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
        let observation = match proxy.query_managed_sessions().await {
            Ok(observation) => observation,
            Err(error) => {
                return Ok(Some(ReconcileOutcome::control_error(
                    error_actual(completion.completed_terminate_epoch),
                    &error,
                )));
            }
        };
        match observation.contest.state {
            GraphicalSessionState::Ambiguous | GraphicalSessionState::Error => {
                Ok(Some(ReconcileOutcome::idle(observation_actual(
                    &observation,
                    completion.completed_terminate_epoch,
                ))))
            }
            GraphicalSessionState::None => {
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
                check_cancellation(cancellation)?;
                completion.completed_terminate_epoch = Some(epoch);
                completion.pending = None;
                persist_completion(&self.artifact_path, completion)?;
                Ok(None)
            }
        }
    }

    pub(super) async fn observe(&self) -> Result<SessionControlActualState, SnapshotError> {
        let completion = match read_completion(&self.artifact_path) {
            CompletionState::Absent => None,
            CompletionState::Valid(completion) if completion.pending.is_none() => {
                completion.completed_terminate_epoch
            }
            CompletionState::Valid(completion) => {
                return Ok(terminating_actual(completion.completed_terminate_epoch));
            }
            CompletionState::Failed => return Ok(error_actual(None)),
        };
        let observation = self
            .proxy()
            .await?
            .query_managed_sessions()
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        Ok(observation_actual(&observation, completion))
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

fn may_resume_pending(pending: &PendingTermination, target_epoch: Option<u64>) -> bool {
    target_epoch.is_some_and(|target| pending.terminate_epoch <= target)
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
        // TODO(R3): combine fresh waiting observations with the exact Agent frame/lease.
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

    pub(in crate::reconcile) fn reconciler(
        directory: &TempDir,
        connection: zbus::Connection,
    ) -> SessionReconciler {
        SessionReconciler {
            connection,
            artifact_path: directory.path().join("session-completion.json"),
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
    fn pending_termination_requires_a_matching_or_newer_target() {
        let pending = PendingTermination {
            terminate_epoch: 7,
            session: GraphicalSession {
                logind_session_id: "c2".to_owned(),
                boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
            },
        };

        assert!(may_resume_pending(&pending, Some(7)));
        assert!(may_resume_pending(&pending, Some(8)));
        assert!(!may_resume_pending(&pending, Some(6)));
        assert!(!may_resume_pending(&pending, None));
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
