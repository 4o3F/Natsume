use std::{fs, path::PathBuf};

use natsume_device_protocol::generated::{
    LockState, SessionControlActualState, SessionControlTarget, SessionState,
};
use natsume_local_control_api::{
    ContestSessionObservation, ContestSessionState, GraphicalSession, Privileged1Proxy,
    SessionLockLevel,
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::atomic_write::{WritePolicy, atomic_write};

use super::{SnapshotError, check_cancellation, invalid_epoch};

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
    desired_lock: SessionLockLevel,
    terminate_epoch: Option<u64>,
}

pub(super) fn validate_target(target: SessionControlTarget) -> Option<ValidatedSessionTarget> {
    let desired_lock = match LockState::try_from(target.lock_state).ok()? {
        LockState::Unlocked => SessionLockLevel::Unlocked,
        LockState::Locked => SessionLockLevel::Locked,
        LockState::Unspecified => return None,
    };
    if target.terminate_epoch.is_some_and(invalid_epoch) {
        return None;
    }
    Some(ValidatedSessionTarget {
        desired_lock,
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
    ) -> Result<SessionControlActualState, SnapshotError> {
        let mut completion = match read_completion(&self.artifact_path) {
            CompletionState::Valid(completion) => completion,
            CompletionState::Absent => SessionCompletionArtifact {
                format_version: ARTIFACT_FORMAT_VERSION,
                completed_terminate_epoch: None,
                pending: None,
            },
            CompletionState::Failed => return Ok(error_actual(None)),
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
        if let Some(actual) = apply_lock(
            &proxy,
            target.desired_lock,
            completion.completed_terminate_epoch,
            cancellation,
        )
        .await?
        {
            return Ok(actual);
        }
        self.observe().await
    }

    async fn resume_pending(
        &self,
        proxy: &Privileged1Proxy<'_>,
        completion: &mut SessionCompletionArtifact,
        target_epoch: Option<u64>,
        cancellation: &CancellationToken,
    ) -> Result<Option<SessionControlActualState>, SnapshotError> {
        let Some(pending) = completion.pending.as_ref() else {
            return Ok(None);
        };
        if !may_resume_pending(pending, target_epoch) {
            return Ok(Some(error_actual(completion.completed_terminate_epoch)));
        }
        check_cancellation(cancellation)?;
        if proxy
            .terminate_contest_session(&pending.session)
            .await
            .is_err()
        {
            return Ok(Some(terminating_actual(
                completion.completed_terminate_epoch,
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
    ) -> Result<Option<SessionControlActualState>, SnapshotError> {
        let Some(epoch) = target_epoch.filter(|epoch| {
            completion
                .completed_terminate_epoch
                .is_none_or(|completed| completed < *epoch)
        }) else {
            return Ok(None);
        };
        check_cancellation(cancellation)?;
        let observation = proxy
            .query_contest_session()
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        match observation.state {
            ContestSessionState::Ambiguous => Ok(Some(SessionControlActualState {
                session_state: SessionState::Ambiguous.into(),
                completed_terminate_epoch: completion.completed_terminate_epoch,
            })),
            ContestSessionState::None => {
                completion.completed_terminate_epoch = Some(epoch);
                persist_completion(&self.artifact_path, completion)?;
                Ok(None)
            }
            ContestSessionState::Active | ContestSessionState::Locked => {
                let Some(session) = observation.session.as_ref() else {
                    return Ok(Some(error_actual(completion.completed_terminate_epoch)));
                };
                completion.pending = Some(PendingTermination {
                    terminate_epoch: epoch,
                    session: session.clone(),
                });
                persist_completion(&self.artifact_path, completion)?;
                check_cancellation(cancellation)?;
                if proxy.terminate_contest_session(session).await.is_err() {
                    return Ok(Some(terminating_actual(
                        completion.completed_terminate_epoch,
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
            .query_contest_session()
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        Ok(observation_actual(&observation, completion))
    }
}

async fn apply_lock(
    proxy: &Privileged1Proxy<'_>,
    desired: SessionLockLevel,
    completed_epoch: Option<u64>,
    cancellation: &CancellationToken,
) -> Result<Option<SessionControlActualState>, SnapshotError> {
    check_cancellation(cancellation)?;
    let observation = proxy
        .query_contest_session()
        .await
        .map_err(|_| SnapshotError::LocalControl)?;
    let Some(session) = observation.session.as_ref() else {
        return Ok(Some(observation_actual(&observation, completed_epoch)));
    };
    let current = match observation.state {
        ContestSessionState::Active => SessionLockLevel::Unlocked,
        ContestSessionState::Locked => SessionLockLevel::Locked,
        ContestSessionState::None | ContestSessionState::Ambiguous => {
            return Ok(Some(observation_actual(&observation, completed_epoch)));
        }
    };
    if current == desired {
        return Ok(Some(observation_actual(&observation, completed_epoch)));
    }
    check_cancellation(cancellation)?;
    if proxy
        .set_contest_session_lock(session, desired)
        .await
        .is_err()
    {
        return Ok(Some(error_actual(completed_epoch)));
    }
    Ok(None)
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
    observation: &ContestSessionObservation,
    completed_terminate_epoch: Option<u64>,
) -> SessionControlActualState {
    let state = match observation.state {
        ContestSessionState::None => SessionState::None,
        ContestSessionState::Active => SessionState::Active,
        ContestSessionState::Locked => SessionState::Locked,
        ContestSessionState::Ambiguous => SessionState::Ambiguous,
    };
    SessionControlActualState {
        session_state: state.into(),
        completed_terminate_epoch,
    }
}

fn terminating_actual(completed_terminate_epoch: Option<u64>) -> SessionControlActualState {
    SessionControlActualState {
        session_state: SessionState::Terminating.into(),
        completed_terminate_epoch,
    }
}

fn error_actual(completed_terminate_epoch: Option<u64>) -> SessionControlActualState {
    SessionControlActualState {
        session_state: SessionState::Error.into(),
        completed_terminate_epoch,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

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
}
