use std::{
    fs::{File, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, ensure};

use super::{Config, IoSnafu, JournalSnafu, Result, WorkspaceSnafu, wire};
use crate::workspace::Workspace;

const JOURNAL_LIMIT: u64 = 64 * 1024;

#[cfg(test)]
#[path = "../../tests/unit/initialization_store.rs"]
mod tests;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub prepared: String,
    pub initialization_id: String,
    pub manifest_sha256: String,
    pub request_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Phase {
    Unpaired,
    Paired,
    Saved { receipt: Receipt },
    Completed { receipt: Receipt },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    format_version: u32,
    origin: String,
    target: PathBuf,
    pub client_id: String,
    pub source_id: Option<String>,
    pub phase: Phase,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    state: State,
    sha256: String,
}

pub(super) struct Store {
    pub root: PathBuf,
    pub state: State,
    journal: std::sync::Arc<File>,
    lock: std::sync::Arc<File>,
}
impl Store {
    pub fn open(config: &Config) -> Result<Self> {
        let target = &config.workspace;
        ensure!(
            target.is_absolute()
                && !target
                    .components()
                    .any(|c| matches!(c, Component::ParentDir)),
            super::PathSnafu
        );
        let parent = std::fs::canonicalize(target.parent().ok_or_else(|| JournalSnafu.build())?)
            .context(IoSnafu)?;
        let name = target.file_name().ok_or_else(|| JournalSnafu.build())?;
        let target = parent.join(name);
        let mut staging_name = std::ffi::OsString::from(".");
        staging_name.push(name);
        staging_name.push(".photo-init");
        let root = parent.join(staging_name);
        // Lock before creating the recovery directory or its first record.
        // Keep this inode permanently; unlinking a lock file splits ownership.
        let lock_path = root.with_extension("photo-init.lock");
        let lock = lock_initialization(&lock_path)?;
        match std::fs::create_dir(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(source).context(IoSnafu),
        }
        ensure!(
            std::fs::canonicalize(&root).context(IoSnafu)? == root,
            JournalSnafu
        );
        let path = root.join("initialization.jsonl");
        let fresh = !path.exists() || std::fs::metadata(&path).context(IoSnafu)?.len() == 0;
        if fresh {
            // Only an untouched directory may recover an absent first record.
            // Pairing markers, prepared data or any other entries forbid reset.
            for entry in std::fs::read_dir(&root).context(IoSnafu)? {
                ensure!(entry.context(IoSnafu)?.path() == path, JournalSnafu);
            }
        }
        if path.exists() {
            ensure!(
                !std::fs::symlink_metadata(&path)
                    .context(IoSnafu)?
                    .is_symlink(),
                JournalSnafu
            );
        }
        let mut journal = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .context(IoSnafu)?;
        let state = if fresh {
            ensure!(!target.exists(), JournalSnafu);
            State {
                format_version: 1,
                origin: config.server_url.clone(),
                target: target.clone(),
                client_id: uuid::Uuid::now_v7().to_string(),
                source_id: None,
                phase: Phase::Unpaired,
            }
        } else {
            read_state(&mut journal)?
        };
        ensure!(
            state.format_version == 1
                && state.origin == config.server_url
                && state.target == target
                && wire::id(&state.client_id),
            JournalSnafu
        );
        match &state.phase {
            Phase::Unpaired => ensure!(state.source_id.is_none(), JournalSnafu),
            _ => ensure!(
                state.source_id.as_deref().is_some_and(wire::id),
                JournalSnafu
            ),
        }
        if let Phase::Saved { receipt } | Phase::Completed { receipt } = &state.phase {
            ensure!(
                wire::id(&receipt.prepared)
                    && wire::id(&receipt.initialization_id)
                    && wire::id(&receipt.request_id)
                    && wire::hash(&receipt.manifest_sha256),
                JournalSnafu
            );
        }
        let store = Self {
            root,
            state,
            journal: std::sync::Arc::new(journal),
            lock: std::sync::Arc::new(lock),
        };
        if fresh {
            store.append()?;
            validate_directory(&parent)?;
        }
        Ok(store)
    }

    // A cancelled async caller must not release ownership while blocking I/O
    // can still append or publish. The task holds the same locked handle.
    pub async fn blocking<T: Send + 'static>(
        &self,
        operation: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let ownership = std::sync::Arc::clone(&self.lock);
        tokio::task::spawn_blocking(move || {
            let _ownership = ownership;
            operation()
        })
        .await
        .context(super::WorkerSnafu)?
    }
    fn append(&self) -> Result<()> {
        append(&self.journal, &self.root, &self.state)
    }
    pub async fn advance(&mut self, phase: Phase) -> Result<()> {
        let mut state = self.state.clone();
        state.phase = phase;
        let written = state.clone();
        let journal = std::sync::Arc::clone(&self.journal);
        let root = self.root.clone();
        self.blocking(move || append(&journal, &root, &written))
            .await?;
        self.state = state;
        Ok(())
    }
    async fn workspace_path(&self, receipt: &Receipt) -> Result<PathBuf> {
        let staged = self.root.join(&receipt.prepared);
        let target = self.state.target.clone();
        self.blocking(move || Ok(if staged.exists() { staged } else { target }))
            .await
    }
    pub async fn verify(
        &self,
        receipt: &Receipt,
        secrets: &impl super::Secrets,
    ) -> Result<Workspace> {
        let path = self.workspace_path(receipt).await?;
        let workspace = Workspace::open_for_verification(&path, true)
            .await
            .context(WorkspaceSnafu)?;
        let identity = super::doctor::verify(&workspace, secrets).await?;
        ensure!(
            identity.client_id == self.state.client_id
                && identity.server_url == self.state.origin
                && Some(&identity.source_id) == self.state.source_id.as_ref()
                && identity.initialization_id == receipt.initialization_id
                && identity.manifest_sha256 == receipt.manifest_sha256,
            super::AuthorizationSnafu
        );
        Ok(workspace)
    }
    pub async fn publish(
        &self,
        receipt: &Receipt,
        secrets: &impl super::Secrets,
    ) -> Result<Workspace> {
        drop(self.verify(receipt, secrets).await?);
        let staged = self.root.join(&receipt.prepared);
        let root = self.root.clone();
        let target = self.state.target.clone();
        self.blocking(move || {
            if staged.exists() {
                ensure!(!target.exists(), JournalSnafu);
                std::fs::rename(staged, &target).context(IoSnafu)?;
                validate_directory(&root)?;
                validate_directory(target.parent().ok_or_else(|| JournalSnafu.build())?)?;
            }
            Ok(())
        })
        .await?;
        Workspace::open_for_verification(&self.state.target, false)
            .await
            .context(WorkspaceSnafu)
    }
}

fn lock_initialization(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .context(IoSnafu)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => super::BusySnafu.fail(),
        Err(std::fs::TryLockError::Error(source)) => Err(source).context(IoSnafu),
    }
}

fn append(mut journal: &File, root: &Path, state: &State) -> Result<()> {
    let encoded = serde_json::to_vec(state).map_err(super::json_error)?;
    let mut line = serde_json::to_vec(&Record {
        state: state.clone(),
        sha256: wire::digest(&encoded),
    })
    .map_err(super::json_error)?;
    line.push(b'\n');
    ensure!(
        journal.metadata().context(IoSnafu)?.len() + line.len() as u64 <= JOURNAL_LIMIT,
        JournalSnafu
    );
    journal.write_all(&line).context(IoSnafu)?;
    journal.sync_all().context(IoSnafu)?;
    validate_directory(root)
}

fn read_state(journal: &mut File) -> Result<State> {
    let mut bytes = Vec::new();
    std::io::Read::by_ref(journal)
        .take(JOURNAL_LIMIT + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu)?;
    ensure!(bytes.len() as u64 <= JOURNAL_LIMIT, JournalSnafu);
    let end = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .ok_or_else(|| JournalSnafu.build())?
        + 1;
    let mut state = None;
    for line in bytes[..end]
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
    {
        let record: Record = serde_json::from_slice(line).map_err(super::json_error)?;
        ensure!(
            record.sha256
                == wire::digest(&serde_json::to_vec(&record.state).map_err(super::json_error)?),
            JournalSnafu
        );
        state = Some(record.state);
    }
    // A power loss may leave only the last append incomplete; retain all committed lines.
    journal.set_len(end as u64).context(IoSnafu)?;
    journal.seek(SeekFrom::End(0)).context(IoSnafu)?;
    journal.sync_all().context(IoSnafu)?;
    state.ok_or_else(|| JournalSnafu.build())
}
pub(super) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)
        .context(IoSnafu)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu)?;
    ensure!(bytes.len() as u64 <= limit, JournalSnafu);
    Ok(bytes)
}
pub(super) fn save_new_or_verify(path: &Path, bytes: &[u8]) -> Result<()> {
    match File::create_new(path) {
        Ok(mut file) => {
            file.write_all(bytes).context(IoSnafu)?;
            file.sync_all().context(IoSnafu)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => ensure!(
            read_bounded(path, bytes.len() as u64)? == bytes,
            JournalSnafu
        ),
        Err(source) => return Err(source).context(IoSnafu),
    }
    validate_directory(path.parent().ok_or_else(|| JournalSnafu.build())?)
}
pub(super) fn validate_directory(path: &Path) -> Result<()> {
    ensure!(
        std::fs::metadata(path).context(IoSnafu)?.is_dir(),
        JournalSnafu
    );
    Ok(())
}
