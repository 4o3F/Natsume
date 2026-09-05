use std::{fs, path::PathBuf};

use natsume_device_protocol::generated::{RuntimeConfigActualState, RuntimeConfigState};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::atomic_write::{WritePolicy, atomic_write};

use super::{SnapshotError, check_cancellation};

const ARTIFACT_FORMAT_VERSION: u32 = 1;

/// Durable activated Runtime Config. It contains only the canonical public `DOMjudge` origin.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeConfigArtifact {
    format_version: u32,
    domjudge_origin: String,
}

/// Runtime Config reconciler backed by one atomically replaced non-secret artifact.
pub(super) struct RuntimeReconciler {
    artifact_path: PathBuf,
}

impl RuntimeReconciler {
    pub(super) fn production() -> Self {
        Self {
            artifact_path: PathBuf::from("/var/lib/natsume/state/runtime-config.json"),
        }
    }

    pub(super) fn reconcile(
        &self,
        domjudge_origin: &str,
        cancellation: &CancellationToken,
    ) -> Result<RuntimeConfigActualState, SnapshotError> {
        check_cancellation(cancellation)?;
        if let ArtifactState::Applied(applied) = read_artifact(&self.artifact_path)
            && applied == domjudge_origin
        {
            return Ok(applied_actual(applied));
        }
        let artifact = RuntimeConfigArtifact {
            format_version: ARTIFACT_FORMAT_VERSION,
            domjudge_origin: domjudge_origin.to_owned(),
        };
        let encoded = serde_json::to_vec(&artifact).map_err(|_| SnapshotError::Artifact)?;
        if atomic_write(&self.artifact_path, &encoded, 0o600, WritePolicy::Replace).is_err() {
            let previous = match read_artifact(&self.artifact_path) {
                ArtifactState::Applied(origin) => Some(origin),
                ArtifactState::Absent | ArtifactState::Failed => None,
            };
            return Ok(failed_actual(previous));
        }
        Ok(self.observe())
    }

    pub(super) fn observe(&self) -> RuntimeConfigActualState {
        match read_artifact(&self.artifact_path) {
            ArtifactState::Absent => RuntimeConfigActualState {
                state: RuntimeConfigState::Absent.into(),
                applied_domjudge_origin: None,
            },
            ArtifactState::Applied(origin) => applied_actual(origin),
            ArtifactState::Failed => failed_actual(None),
        }
    }
}

fn applied_actual(origin: String) -> RuntimeConfigActualState {
    RuntimeConfigActualState {
        state: RuntimeConfigState::Applied.into(),
        applied_domjudge_origin: Some(origin),
    }
}

enum ArtifactState {
    Absent,
    Applied(String),
    Failed,
}

fn read_artifact(path: &std::path::Path) -> ArtifactState {
    let encoded = match fs::read(path) {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ArtifactState::Absent,
        Err(_) => return ArtifactState::Failed,
    };
    let Ok(artifact) = serde_json::from_slice::<RuntimeConfigArtifact>(&encoded) else {
        return ArtifactState::Failed;
    };
    if artifact.format_version != ARTIFACT_FORMAT_VERSION
        || !is_canonical_https_origin(&artifact.domjudge_origin)
    {
        return ArtifactState::Failed;
    }
    ArtifactState::Applied(artifact.domjudge_origin)
}

pub(super) fn is_canonical_https_origin(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.host().is_some()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url.origin().ascii_serialization() == value
}

fn failed_actual(previous: Option<String>) -> RuntimeConfigActualState {
    RuntimeConfigActualState {
        state: RuntimeConfigState::Failed.into(),
        applied_domjudge_origin: previous,
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt as _;

    use tempfile::TempDir;

    use super::*;

    fn reconciler(directory: &TempDir) -> RuntimeReconciler {
        RuntimeReconciler {
            artifact_path: directory.path().join("runtime-config.json"),
        }
    }

    #[test]
    fn runtime_reconcile_persists_and_exact_replay_does_not_rewrite() {
        let directory = TempDir::new()
            .unwrap_or_else(|error| panic!("test directory must be created: {error}"));
        let reconciler = reconciler(&directory);
        let applied = reconciler
            .reconcile("https://judge.example", &CancellationToken::new())
            .unwrap_or_else(|error| panic!("runtime reconcile must succeed: {error}"));
        let inode = fs::metadata(&reconciler.artifact_path)
            .unwrap_or_else(|error| panic!("runtime metadata must load: {error}"))
            .ino();
        let replay = reconciler
            .reconcile("https://judge.example", &CancellationToken::new())
            .unwrap_or_else(|error| panic!("runtime replay must succeed: {error}"));
        assert_eq!(applied, replay);
        assert_eq!(
            fs::metadata(&reconciler.artifact_path)
                .unwrap_or_else(|error| panic!("runtime metadata must reload: {error}"))
                .ino(),
            inode
        );
        assert_eq!(applied.state, i32::from(RuntimeConfigState::Applied));
        assert_eq!(
            applied.applied_domjudge_origin.as_deref(),
            Some("https://judge.example")
        );
    }

    #[test]
    fn runtime_origin_must_be_one_canonical_https_origin() {
        assert!(is_canonical_https_origin("https://judge.example"));
        for invalid in [
            "http://judge.example/",
            "https://judge.example/",
            "https://user@judge.example/",
            "https://judge.example/path",
            "https://judge.example/?query",
            "https://judge.example/#fragment",
            "https://JUDGE.example/",
        ] {
            assert!(!is_canonical_https_origin(invalid), "accepted {invalid}");
        }
    }
}
