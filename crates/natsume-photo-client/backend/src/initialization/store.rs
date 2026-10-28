use std::{
    fs::{File, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, ensure};

use super::{
    Config, InvalidSnafu, IoSnafu, JsonSnafu, Result, WorkspaceSnafu,
    wire::{self, Manifest},
};
use crate::workspace::Workspace;

const JOURNAL_LIMIT: u64 = 64 * 1024;

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
    journal: File,
}
impl Store {
    pub fn open(config: &Config) -> Result<Self> {
        let target = &config.workspace;
        ensure!(
            target.is_absolute()
                && !target
                    .components()
                    .any(|c| matches!(c, Component::ParentDir)),
            InvalidSnafu
        );
        let parent = std::fs::canonicalize(target.parent().ok_or_else(|| InvalidSnafu.build())?)
            .context(IoSnafu)?;
        let name = target.file_name().ok_or_else(|| InvalidSnafu.build())?;
        let target = parent.join(name);
        let mut staging_name = std::ffi::OsString::from(".");
        staging_name.push(name);
        staging_name.push(".photo-init");
        let root = parent.join(staging_name);
        let fresh = match std::fs::create_dir(&root) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(source) => return Err(source).context(IoSnafu),
        };
        ensure!(
            std::fs::canonicalize(&root).context(IoSnafu)? == root,
            InvalidSnafu
        );
        let path = root.join("initialization.jsonl");
        let journal = if fresh {
            File::create_new(&path).context(IoSnafu)?
        } else {
            ensure!(
                !std::fs::symlink_metadata(&path)
                    .context(IoSnafu)?
                    .is_symlink(),
                InvalidSnafu
            );
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .context(IoSnafu)?
        };
        // Reopen create_new's write-only handle for journal validation on every run.
        drop(journal);
        let mut journal = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .context(IoSnafu)?;
        match journal.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return super::BusySnafu.fail(),
            Err(std::fs::TryLockError::Error(source)) => return Err(source).context(IoSnafu),
        }
        let state = if fresh {
            ensure!(!target.exists(), InvalidSnafu);
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
            InvalidSnafu
        );
        match &state.phase {
            Phase::Unpaired => ensure!(state.source_id.is_none(), InvalidSnafu),
            _ => ensure!(
                state.source_id.as_deref().is_some_and(wire::id),
                InvalidSnafu
            ),
        }
        if let Phase::Saved { receipt } | Phase::Completed { receipt } = &state.phase {
            ensure!(
                wire::id(&receipt.prepared)
                    && wire::id(&receipt.initialization_id)
                    && wire::id(&receipt.request_id)
                    && wire::hash(&receipt.manifest_sha256),
                InvalidSnafu
            );
        }
        let mut store = Self {
            root,
            state,
            journal,
        };
        if fresh {
            store.append()?;
            validate_directory(&parent)?;
        }
        Ok(store)
    }

    fn append(&mut self) -> Result<()> {
        let encoded = serde_json::to_vec(&self.state).context(JsonSnafu)?;
        let mut line = serde_json::to_vec(&Record {
            state: self.state.clone(),
            sha256: wire::digest(&encoded),
        })
        .context(JsonSnafu)?;
        line.push(b'\n');
        ensure!(
            self.journal.metadata().context(IoSnafu)?.len() + line.len() as u64 <= JOURNAL_LIMIT,
            InvalidSnafu
        );
        self.journal.write_all(&line).context(IoSnafu)?;
        self.journal.sync_all().context(IoSnafu)?;
        validate_directory(&self.root)
    }
    pub fn advance(&mut self, phase: Phase) -> Result<()> {
        self.state.phase = phase;
        self.append()
    }
    fn workspace_path(&self, receipt: &Receipt) -> PathBuf {
        let staged = self.root.join(&receipt.prepared);
        if staged.exists() {
            staged
        } else {
            self.state.target.clone()
        }
    }
    pub async fn verify(&self, receipt: &Receipt) -> Result<Workspace> {
        let path = self.workspace_path(receipt);
        let bytes = read_bounded(
            &path.join("initialization.json"),
            wire::MANIFEST_LIMIT as u64,
        )?;
        ensure!(
            wire::digest(&bytes) == receipt.manifest_sha256,
            InvalidSnafu
        );
        let manifest: Manifest = serde_json::from_slice(&bytes).context(JsonSnafu)?;
        manifest.validate(
            self.state
                .source_id
                .as_deref()
                .ok_or_else(|| InvalidSnafu.build())?,
        )?;
        ensure!(
            manifest.initialization_id == receipt.initialization_id,
            InvalidSnafu
        );
        let identity: serde_json::Value =
            serde_json::from_slice(&read_bounded(&path.join("authorization.json"), 8192)?)
                .context(JsonSnafu)?;
        ensure!(
            identity["client_id"] == self.state.client_id
                && identity["server_url"] == self.state.origin
                && identity["source_id"] == manifest.source_id
                && identity["dataset_id"] == manifest.dataset_id
                && identity["initialization_id"] == receipt.initialization_id
                && identity["manifest_sha256"] == receipt.manifest_sha256,
            InvalidSnafu
        );
        let workspace = Workspace::open(&path).await.context(WorkspaceSnafu)?;
        let roster = workspace.roster().await.context(WorkspaceSnafu)?;
        let actual: std::collections::BTreeMap<_, _> =
            roster.teams().iter().map(|t| (&t.account_id, t)).collect();
        let expected: std::collections::BTreeMap<_, _> =
            manifest.teams.iter().map(|t| (&t.account_id, t)).collect();
        ensure!(actual == expected, InvalidSnafu);
        for team in &manifest.teams {
            let logo = workspace
                .logo_path(&team.account_id)
                .await
                .context(WorkspaceSnafu)?;
            let expected = manifest
                .school_logos
                .iter()
                .find(|l| l.organization_id == team.organization_id);
            match (logo, expected) {
                (None, None) => {}
                (Some(path), Some(expected)) => ensure!(
                    wire::digest(&read_bounded(&path, wire::IMAGE_LIMIT as u64)?)
                        == expected.sha256,
                    InvalidSnafu
                ),
                _ => return InvalidSnafu.fail(),
            }
        }
        Ok(workspace)
    }
    pub async fn publish(&self, receipt: &Receipt) -> Result<Workspace> {
        drop(self.verify(receipt).await?);
        let staged = self.root.join(&receipt.prepared);
        if staged.exists() {
            ensure!(!self.state.target.exists(), InvalidSnafu);
            std::fs::rename(staged, &self.state.target).context(IoSnafu)?;
            validate_directory(&self.root)?;
            validate_directory(
                self.state
                    .target
                    .parent()
                    .ok_or_else(|| InvalidSnafu.build())?,
            )?;
        }
        self.verify(receipt).await
    }
}

fn read_state(journal: &mut File) -> Result<State> {
    let mut bytes = Vec::new();
    std::io::Read::by_ref(journal)
        .take(JOURNAL_LIMIT + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu)?;
    ensure!(bytes.len() as u64 <= JOURNAL_LIMIT, InvalidSnafu);
    let end = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .ok_or_else(|| InvalidSnafu.build())?
        + 1;
    let mut state = None;
    for line in bytes[..end]
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
    {
        let record: Record = serde_json::from_slice(line).context(JsonSnafu)?;
        ensure!(
            record.sha256 == wire::digest(&serde_json::to_vec(&record.state).context(JsonSnafu)?),
            InvalidSnafu
        );
        state = Some(record.state);
    }
    // A power loss may leave only the last append incomplete; retain all committed lines.
    journal.set_len(end as u64).context(IoSnafu)?;
    journal.seek(SeekFrom::End(0)).context(IoSnafu)?;
    journal.sync_all().context(IoSnafu)?;
    state.ok_or_else(|| InvalidSnafu.build())
}
pub(super) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)
        .context(IoSnafu)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu)?;
    ensure!(bytes.len() as u64 <= limit, InvalidSnafu);
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
            InvalidSnafu
        ),
        Err(source) => return Err(source).context(IoSnafu),
    }
    validate_directory(path.parent().ok_or_else(|| InvalidSnafu.build())?)
}
pub(super) fn validate_directory(path: &Path) -> Result<()> {
    ensure!(
        std::fs::metadata(path).context(IoSnafu)?.is_dir(),
        InvalidSnafu
    );
    Ok(())
}
