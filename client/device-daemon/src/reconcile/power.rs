use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use natsume_device_protocol::generated::{PowerControlActualState, PowerControlTarget, PowerState};
use natsume_local_control_api::Privileged1Proxy;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{ReconcileOutcome, SnapshotError, check_cancellation};
use crate::atomic_write::{WritePolicy, atomic_write};

#[derive(Clone, Copy, PartialEq)]
pub(super) struct ValidatedPowerTarget {
    epoch: Option<u64>,
    deadline: Option<i64>,
}
pub(super) fn validate_target(target: PowerControlTarget) -> Option<ValidatedPowerTarget> {
    match (target.shutdown_epoch, target.expires_at_unix_ms) {
        (None, None) => Some(ValidatedPowerTarget {
            epoch: None,
            deadline: None,
        }),
        (Some(epoch), Some(deadline)) if epoch > 0 && deadline > 0 => Some(ValidatedPowerTarget {
            epoch: Some(epoch),
            deadline: Some(deadline),
        }),
        _ => None,
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    epoch: u64,
    state: String,
}

pub(super) struct PowerReconciler {
    connection: zbus::Connection,
    artifact_path: PathBuf,
}
impl PowerReconciler {
    pub(super) fn production(connection: zbus::Connection) -> Self {
        Self {
            connection,
            artifact_path: "/var/lib/natsume/state/power-control.json".into(),
        }
    }
    pub(super) async fn reconcile(
        &self,
        target: ValidatedPowerTarget,
        cancellation: &CancellationToken,
    ) -> Result<ReconcileOutcome<PowerControlActualState>, SnapshotError> {
        let prior = read(&self.artifact_path)?;
        let Some(epoch) = target.epoch else {
            return Ok(ReconcileOutcome {
                actual: actual(PowerState::Idle, prior.map(|a| a.epoch)),
                retry: false,
            });
        };
        if let Some(prior) = prior.as_ref().filter(|value| value.epoch >= epoch) {
            return Ok(ReconcileOutcome {
                actual: actual(
                    if prior.state == "accepted" {
                        PowerState::Accepted
                    } else {
                        PowerState::Error
                    },
                    Some(epoch),
                ),
                retry: false,
            });
        }
        if unix_ms().ok_or(SnapshotError::Artifact)?
            > target
                .deadline
                .ok_or(SnapshotError::InvalidServerSnapshot)?
        {
            return Ok(ReconcileOutcome {
                actual: actual(PowerState::Expired, Some(epoch)),
                retry: false,
            });
        }
        check_cancellation(cancellation)?;
        persist(
            &self.artifact_path,
            &Artifact {
                epoch,
                state: "pending".to_owned(),
            },
        )?;
        let proxy = Privileged1Proxy::new(&self.connection)
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        if let Ok(()) = proxy.request_power_off().await {
            persist(
                &self.artifact_path,
                &Artifact {
                    epoch,
                    state: "accepted".to_owned(),
                },
            )?;
            Ok(ReconcileOutcome {
                actual: actual(PowerState::Accepted, Some(epoch)),
                retry: false,
            })
        } else {
            persist(
                &self.artifact_path,
                &Artifact {
                    epoch,
                    state: "error".to_owned(),
                },
            )?;
            Ok(ReconcileOutcome {
                actual: actual(PowerState::Error, Some(epoch)),
                retry: false,
            })
        }
    }
    pub(super) fn observe(&self) -> PowerControlActualState {
        read(&self.artifact_path).ok().flatten().map_or_else(
            || actual(PowerState::Idle, None),
            |artifact| {
                actual(
                    if artifact.state == "accepted" {
                        PowerState::Accepted
                    } else {
                        PowerState::Error
                    },
                    Some(artifact.epoch),
                )
            },
        )
    }
}
fn actual(state: PowerState, epoch: Option<u64>) -> PowerControlActualState {
    PowerControlActualState {
        state: state.into(),
        attempted_shutdown_epoch: epoch,
    }
}
fn unix_ms() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_millis()).ok())
}
fn read(path: &std::path::Path) -> Result<Option<Artifact>, SnapshotError> {
    match fs::read(path) {
        Ok(value) => serde_json::from_slice(&value)
            .map(Some)
            .map_err(|_| SnapshotError::Artifact),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(SnapshotError::Artifact),
    }
}
fn persist(path: &std::path::Path, artifact: &Artifact) -> Result<(), SnapshotError> {
    let value = serde_json::to_vec(artifact).map_err(|_| SnapshotError::Artifact)?;
    atomic_write(path, &value, 0o600, WritePolicy::Replace).map_err(|_| SnapshotError::Artifact)
}

#[cfg(test)]
pub(in crate::reconcile) fn reconciler(
    directory: &tempfile::TempDir,
    connection: zbus::Connection,
) -> PowerReconciler {
    PowerReconciler {
        connection,
        artifact_path: directory.path().join("power-control.json"),
    }
}
