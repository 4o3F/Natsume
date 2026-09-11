use std::{fs, path::PathBuf};

use natsume_device_protocol::generated::{HomeActualState, HomeState};
use natsume_local_control_api::{
    GraphicalSession, HomeResetPhase, HomeResetProgress, Privileged1Proxy, ResourceControlError,
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::atomic_write::{WritePolicy, atomic_write};

use super::{ReconcileOutcome, SnapshotError, check_cancellation, invalid_epoch};

const ARTIFACT_FORMAT_VERSION: u32 = 1;

/// Daemon-side durable publication barrier for the latest verified Home reset.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HomeCompletionArtifact {
    format_version: u32,
    completed_reset_epoch: u64,
}

/// Home reconciler which drives the helper's fixed Prepare/Apply/Verify/Recover protocol.
pub(super) struct HomeReconciler {
    connection: zbus::Connection,
    artifact_path: PathBuf,
}

impl HomeReconciler {
    pub(super) fn production(connection: zbus::Connection) -> Self {
        Self {
            connection,
            artifact_path: PathBuf::from("/var/lib/natsume/state/home-completion.json"),
        }
    }

    async fn proxy(&self) -> Result<Privileged1Proxy<'_>, SnapshotError> {
        Privileged1Proxy::new(&self.connection)
            .await
            .map_err(|_| SnapshotError::LocalControl)
    }

    pub(super) async fn reconcile(
        &self,
        target_epoch: Option<u64>,
        waiting: Option<&GraphicalSession>,
        cancellation: &CancellationToken,
    ) -> Result<ReconcileOutcome<HomeActualState>, SnapshotError> {
        let mut completed = match read_completion(&self.artifact_path) {
            CompletionState::Absent => None,
            CompletionState::Valid(epoch) => Some(epoch),
            CompletionState::Failed => return Ok(ReconcileOutcome::idle(recovery_required(None))),
        };
        let proxy = self.proxy().await?;

        let progress = match proxy.query_home_reset().await {
            Ok(progress) => progress,
            Err(error) => {
                return Ok(ReconcileOutcome::control_error(
                    recovery_required(completed),
                    &error,
                ));
            }
        };
        let mut verified_epoch = None;
        if let Some(progress) = progress {
            // A durable Helper window owns its captured epoch independently of
            // the current transport/Target. Finish it before considering new work.
            let finished = finish_progress(&proxy, &progress).await?;
            if !finished.actual {
                return Ok(ReconcileOutcome {
                    actual: recovery_required(completed),
                    retry: finished.retry,
                });
            }
            verified_epoch = Some(progress.reset_epoch);
            completed = advance_completion(&self.artifact_path, completed, progress.reset_epoch)?;
        }

        let Some(epoch) = target_epoch else {
            if !proxy.is_home_ready().await.unwrap_or(false)
                && let Err(error) = proxy.recover_local_home().await
            {
                return Ok(ReconcileOutcome::control_error(
                    recovery_required(completed),
                    &error,
                ));
            }
            return Ok(ReconcileOutcome::idle(
                if (completed.is_none() || verified_epoch == completed)
                    && proxy.is_home_ready().await.unwrap_or(false)
                {
                    steady(completed)
                } else {
                    recovery_required(completed)
                },
            ));
        };
        if completed.is_some_and(|value| value >= epoch) {
            return Ok(ReconcileOutcome::idle(
                if verified_epoch.is_some_and(|verified| verified >= epoch) {
                    steady(completed)
                } else {
                    recovery_required(completed)
                },
            ));
        }

        check_cancellation(cancellation)?;
        // Only the coordinator can supply a current Agent presentation whose
        // exact waiting session has also been confirmed in the foreground.
        let Some(waiting) = waiting else {
            return Ok(ReconcileOutcome::retry(recovery_required(completed)));
        };
        if let Err(error) = proxy.prepare_home_reset(epoch, waiting).await {
            return Ok(ReconcileOutcome::control_error(
                recovery_required(completed),
                &error,
            ));
        }
        let prepared = HomeResetProgress {
            reset_epoch: epoch,
            phase: HomeResetPhase::Prepared,
        };
        let finished = finish_progress(&proxy, &prepared).await?;
        if !finished.actual {
            return Ok(ReconcileOutcome {
                actual: recovery_required(completed),
                retry: finished.retry,
            });
        }
        let completed = advance_completion(&self.artifact_path, completed, epoch)?;
        Ok(ReconcileOutcome::idle(steady(completed)))
    }

    pub(super) async fn observe(&self) -> Result<HomeActualState, SnapshotError> {
        let completed = match read_completion(&self.artifact_path) {
            CompletionState::Absent => None,
            CompletionState::Valid(epoch) => Some(epoch),
            CompletionState::Failed => return Ok(recovery_required(None)),
        };
        let proxy = self.proxy().await?;
        let Ok(progress) = proxy.query_home_reset().await else {
            return Ok(recovery_required(completed));
        };
        let Some(progress) = progress else {
            return Ok(
                if completed.is_none() && proxy.is_home_ready().await.unwrap_or(false) {
                    steady(None)
                } else {
                    recovery_required(completed)
                },
            );
        };
        let verified = match proxy.verify_home_reset(progress.reset_epoch).await {
            Ok(verified) if verified.reset_epoch == progress.reset_epoch => verified,
            Ok(_) | Err(_) => return Ok(recovery_required(completed)),
        };
        Ok(match verified.phase {
            HomeResetPhase::Verified if Some(verified.reset_epoch) == completed => {
                steady(completed)
            }
            HomeResetPhase::RecoveryRequired | HomeResetPhase::Verified => {
                recovery_required(completed)
            }
            HomeResetPhase::Draining | HomeResetPhase::Prepared | HomeResetPhase::Applied => {
                HomeActualState {
                    state: HomeState::Resetting.into(),
                    completed_reset_epoch: completed,
                }
            }
        })
    }
}

async fn finish_progress(
    proxy: &Privileged1Proxy<'_>,
    progress: &HomeResetProgress,
) -> Result<ReconcileOutcome<bool>, SnapshotError> {
    if invalid_epoch(progress.reset_epoch) {
        return Ok(ReconcileOutcome::idle(false));
    }
    // This epoch is already owned by Helper. A rejected mount/template can be
    // repaired without changing Target or the observed RecoveryRequired state.
    // Keep the existing bounded backoff alive; Helper still checks every retry.
    let pending_error = |error: &ResourceControlError| {
        let mut outcome = ReconcileOutcome::control_error(false, error);
        outcome.retry |= matches!(error, ResourceControlError::Rejected(_));
        outcome
    };
    let applied = match progress.phase {
        HomeResetPhase::Prepared => proxy.apply_home_reset(progress.reset_epoch).await,
        HomeResetPhase::Draining | HomeResetPhase::RecoveryRequired => {
            proxy.recover_home_reset(progress.reset_epoch).await
        }
        HomeResetPhase::Applied | HomeResetPhase::Verified => Ok(()),
    };
    if let Err(error) = applied {
        return Ok(pending_error(&error));
    }
    let verified = match proxy.verify_home_reset(progress.reset_epoch).await {
        Ok(verified) => verified,
        Err(error) => return Ok(pending_error(&error)),
    };
    if verified.reset_epoch != progress.reset_epoch {
        return Ok(ReconcileOutcome::idle(false));
    }
    Ok(if verified.phase == HomeResetPhase::Verified {
        ReconcileOutcome::idle(true)
    } else {
        ReconcileOutcome::retry(false)
    })
}

enum CompletionState {
    Absent,
    Valid(u64),
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
    let Ok(artifact) = serde_json::from_slice::<HomeCompletionArtifact>(&encoded) else {
        return CompletionState::Failed;
    };
    if artifact.format_version != ARTIFACT_FORMAT_VERSION
        || invalid_epoch(artifact.completed_reset_epoch)
    {
        return CompletionState::Failed;
    }
    CompletionState::Valid(artifact.completed_reset_epoch)
}

fn persist_completion(path: &std::path::Path, epoch: u64) -> Result<(), SnapshotError> {
    let artifact = HomeCompletionArtifact {
        format_version: ARTIFACT_FORMAT_VERSION,
        completed_reset_epoch: epoch,
    };
    let encoded = serde_json::to_vec(&artifact).map_err(|_| SnapshotError::Artifact)?;
    atomic_write(path, &encoded, 0o600, WritePolicy::Replace).map_err(|_| SnapshotError::Artifact)
}

fn advance_completion(
    path: &std::path::Path,
    current: Option<u64>,
    verified: u64,
) -> Result<Option<u64>, SnapshotError> {
    if invalid_epoch(verified) {
        return Err(SnapshotError::Artifact);
    }
    if current.is_some_and(|current| current >= verified) {
        return Ok(current);
    }
    persist_completion(path, verified)?;
    Ok(Some(verified))
}

fn steady(completed_reset_epoch: Option<u64>) -> HomeActualState {
    HomeActualState {
        state: HomeState::Steady.into(),
        completed_reset_epoch,
    }
}

fn recovery_required(completed_reset_epoch: Option<u64>) -> HomeActualState {
    HomeActualState {
        state: HomeState::RecoveryRequired.into(),
        completed_reset_epoch,
    }
}

#[cfg(test)]
pub(super) mod tests {
    use tempfile::TempDir;

    use super::*;

    pub(in crate::reconcile) fn reconciler(
        directory: &TempDir,
        connection: zbus::Connection,
    ) -> HomeReconciler {
        HomeReconciler {
            connection,
            artifact_path: directory.path().join("home-completion.json"),
        }
    }

    #[test]
    fn completed_epoch_is_persisted_before_it_can_be_observed() {
        let directory = TempDir::new()
            .unwrap_or_else(|error| panic!("test directory must be created: {error}"));
        let path = directory.path().join("home-completion.json");

        persist_completion(&path, 8)
            .unwrap_or_else(|error| panic!("completion must persist: {error}"));

        assert!(matches!(read_completion(&path), CompletionState::Valid(8)));
    }

    #[test]
    fn corrupt_completion_requests_recovery_without_advancing_epoch() {
        let directory = TempDir::new()
            .unwrap_or_else(|error| panic!("test directory must be created: {error}"));
        let path = directory.path().join("home-completion.json");
        fs::write(&path, b"{\"completed_reset_epoch\":9}")
            .unwrap_or_else(|error| panic!("fixture must be written: {error}"));

        assert!(matches!(read_completion(&path), CompletionState::Failed));
        assert_eq!(recovery_required(None).completed_reset_epoch, None);
    }

    #[test]
    fn completion_never_moves_backwards() {
        let directory = TempDir::new()
            .unwrap_or_else(|error| panic!("test directory must be created: {error}"));
        let path = directory.path().join("home-completion.json");
        persist_completion(&path, 9)
            .unwrap_or_else(|error| panic!("completion must persist: {error}"));

        let completed = advance_completion(&path, Some(9), 8)
            .unwrap_or_else(|error| panic!("older fact must be ignored: {error}"));

        assert_eq!(completed, Some(9));
        assert!(matches!(read_completion(&path), CompletionState::Valid(9)));
    }
}
