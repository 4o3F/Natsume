use std::{
    fs::{File, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, ensure};

use super::{ClientId, identity::PreparedId};
use super::{IoSnafu, JournalSnafu, Result, ServerOrigin, WorkspaceSnafu};
use crate::workspace::{Workspace, WorkspaceReader};
use natsume_photo_protocol::{InitializationId, InitializationRequestId, Sha256Digest, SourceId};

const JOURNAL_LIMIT: u64 = 64 * 1024;

#[cfg(test)]
#[path = "../../tests/unit/initialization_store.rs"]
mod tests;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub prepared: PreparedId,
    pub initialization_id: InitializationId,
    pub manifest_sha256: Sha256Digest,
    pub request_id: InitializationRequestId,
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
    origin: ServerOrigin,
    target: PathBuf,
    pub client_id: ClientId,
    pub source_id: Option<SourceId>,
    pub phase: Phase,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    state: State,
    sha256: Sha256Digest,
}

pub(super) struct Store {
    root: PathBuf,
    state: State,
    journal: std::sync::Arc<File>,
    lock: InitializationLease,
}
// A lease can only be acquired by this module's actual OS-lock operation.
#[derive(Clone)]
pub(super) struct InitializationLease {
    _handle: std::sync::Arc<File>,
}
impl InitializationLease {
    // Cancellation drops the waiter, but the real task retains the locked handle.
    pub async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let ownership = self.clone();
        tokio::task::spawn_blocking(move || {
            let _ownership = ownership;
            operation()
        })
        .await
        .context(super::WorkerSnafu)?
    }
}
impl Store {
    pub fn lease(&self) -> InitializationLease {
        self.lock.clone()
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn state(&self) -> &State {
        &self.state
    }
    pub async fn record_pairing(&mut self, source: SourceId) -> Result<()> {
        ensure!(matches!(self.state.phase, Phase::Unpaired), JournalSnafu);
        let mut next = self.state.clone();
        next.source_id = Some(source);
        next.phase = Phase::Paired;
        self.commit(next).await
    }
    pub fn open(target: &Path, origin: &ServerOrigin) -> Result<Self> {
        ensure!(
            target.is_absolute()
                && !target
                    .components()
                    .any(|c| matches!(c, Component::ParentDir)),
            super::PathSnafu
        );
        let parent =
            std::fs::canonicalize(target.parent().ok_or_else(|| super::PathSnafu.build())?)
                .context(IoSnafu)?;
        let name = target.file_name().ok_or_else(|| super::PathSnafu.build())?;
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
            super::PathSnafu
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
                super::PathSnafu
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
            ensure!(!target.exists(), super::FileConflictSnafu);
            State {
                format_version: 1,
                origin: origin.clone(),
                target: target.clone(),
                client_id: ClientId::generate(),
                source_id: None,
                phase: Phase::Unpaired,
            }
        } else {
            read_state(&mut journal)?
        };
        ensure!(
            state.format_version == 1 && state.origin == *origin && state.target == target,
            JournalSnafu
        );
        match &state.phase {
            Phase::Unpaired => ensure!(state.source_id.is_none(), JournalSnafu),
            _ => ensure!(state.source_id.is_some(), JournalSnafu),
        }
        let store = Self {
            root,
            state,
            journal: std::sync::Arc::new(journal),
            lock: InitializationLease {
                _handle: std::sync::Arc::new(lock),
            },
        };
        if fresh {
            store.append()?;
            validate_directory(&parent)?;
        }
        Ok(store)
    }

    fn append(&self) -> Result<()> {
        append(&self.journal, &self.root, &self.state)
    }
    pub async fn record_saved(&mut self, receipt: Receipt) -> Result<()> {
        ensure!(matches!(self.state.phase, Phase::Paired), JournalSnafu);
        let mut next = self.state.clone();
        next.phase = Phase::Saved { receipt };
        self.commit(next).await
    }
    pub async fn record_completed(&mut self) -> Result<Receipt> {
        let Phase::Saved { receipt } = &self.state.phase else {
            return JournalSnafu.fail();
        };
        let receipt = receipt.clone();
        let mut next = self.state.clone();
        next.phase = Phase::Completed {
            receipt: receipt.clone(),
        };
        self.commit(next).await?;
        Ok(receipt)
    }
    async fn commit(&mut self, state: State) -> Result<()> {
        let written = state.clone();
        let journal = std::sync::Arc::clone(&self.journal);
        let root = self.root.clone();
        self.lock
            .run(move || append(&journal, &root, &written))
            .await?;
        self.state = state;
        Ok(())
    }
    async fn workspace_path(&self, receipt: &Receipt) -> Result<PathBuf> {
        let staged = self.root.join(receipt.prepared.as_str());
        let target = self.state.target.clone();
        self.lock
            .run(move || Ok(if staged.exists() { staged } else { target }))
            .await
    }
    pub async fn verify(
        &self,
        receipt: &Receipt,
        secrets: &impl super::Secrets,
    ) -> Result<WorkspaceReader> {
        let path = self.workspace_path(receipt).await?;
        let workspace = WorkspaceReader::inspect(&path)
            .await
            .context(WorkspaceSnafu)?;
        let identity = super::authorization::verify(&workspace, secrets)
            .await
            .context(super::OfflineAuthorizationSnafu)?;
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
        let verified = self.verify(receipt, secrets).await?;
        self.lock
            .run({
                let target = self.state.target.clone();
                move || Workspace::publish_verified(verified, &target).context(WorkspaceSnafu)
            })
            .await
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
        sha256: Sha256Digest::of(&encoded),
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
                == Sha256Digest::of(&serde_json::to_vec(&record.state).map_err(super::json_error)?),
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
    ensure!(
        bytes.len() as u64 <= limit,
        super::LimitSnafu {
            limit: usize::try_from(limit).unwrap_or(usize::MAX)
        }
    );
    Ok(bytes)
}
pub(super) fn save_new_or_verify(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| super::PathSnafu.build())?;
    let temporary = write_synced_temporary(parent, |file| file.write_all(bytes))?;
    match temporary.persist_noclobber(path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure!(
                read_bounded(path, bytes.len() as u64)? == bytes,
                super::FileConflictSnafu
            );
        }
        Err(error) => return Err(error.error).context(IoSnafu),
    }
    validate_directory(parent)
}

// Only Server-verified downloads use replacement, at a content-addressed cache path.
pub(super) fn save_download(directory: &Path, digest: &Sha256Digest, bytes: &[u8]) -> Result<()> {
    let temporary = write_synced_temporary(directory, |file| file.write_all(bytes))?;
    temporary
        .persist(directory.join(format!("{digest}.png")))
        .map_err(|error| error.error)
        .context(IoSnafu)?;
    validate_directory(directory)
}

fn write_synced_temporary(
    parent: &Path,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> Result<tempfile::NamedTempFile> {
    let mut temporary = tempfile::NamedTempFile::new_in(parent).context(IoSnafu)?;
    write(temporary.as_file_mut()).context(IoSnafu)?;
    temporary.as_file().sync_all().context(IoSnafu)?;
    Ok(temporary)
}
pub(super) fn validate_directory(path: &Path) -> Result<()> {
    ensure!(
        std::fs::metadata(path).context(IoSnafu)?.is_dir(),
        super::NotDirectorySnafu
    );
    Ok(())
}
