//! Tokio-owned ingestion state, safe native notifications and copy-only archives.
use crate::capture::{self, GroupId, Session, TaskId};
use snafu::{IntoError as _, OptionExt as _, ResultExt as _, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use uuid::Uuid;

pub(crate) mod archive;
pub(crate) mod native;
pub use archive::ArchiveError;

pub(crate) const INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum Error {
    #[snafu(display("hot-folder directory operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("native directory monitoring failed"))]
    Monitor { source: windows::core::Error },
    #[snafu(display("cannot read the workspace roster: {source}"))]
    Workspace { source: crate::workspace::Error },
    #[snafu(display("capture transition rejected: {source}"))]
    Capture { source: capture::Error },
    #[snafu(display("source archival failed: {source}"))]
    Archive { source: ArchiveError },
    #[snafu(display("hot folder must be an absolute local directory outside the workspace"))]
    UnsafePath,
    #[snafu(display("a new capture session requires an empty hot folder"))]
    NotEmpty,
    #[snafu(display("directory monitoring has not recovered; group transitions are blocked"))]
    MonitorUnavailable,
    #[snafu(display("photo cannot be assigned to this active group"))]
    NotReviewable,
    #[snafu(display("ingestion state invariant was violated"))]
    Invariant,
    #[snafu(display("ingestion worker has stopped"))]
    Stopped,
    #[snafu(display("ingestion task failed"))]
    Worker { source: tokio::task::JoinError },
}
pub type Result<T> = std::result::Result<T, Error>;

/// Persistent-in-memory worker state, distinct from a failed method call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestionState {
    Running,
    RecoveringMonitor,
    PausedDirectory,
    PausedArchive,
    Stopped,
}

/// A stable source copy, never a decoded candidate or a confirmed final.
#[derive(Debug, Clone)]
pub struct ArchivedPhoto {
    pub id: Uuid,
    pub source: PathBuf,
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    /// None requires explicit photographer review.
    pub group_id: Option<GroupId>,
}
/// A source that vanished before archival, retained for photographer review.
#[derive(Debug, Clone)]
pub struct MissingSource {
    pub source: PathBuf,
    pub group_id: Option<GroupId>,
}
/// Owned operator facts; this is not the contestant presentation state.
#[derive(Debug, Clone)]
pub struct HotFolderSnapshot {
    pub session_id: Uuid,
    pub group_id: Option<GroupId>,
    pub account_id: Option<String>,
    pub pending_tasks: usize,
    pub paused: bool,
    pub state: IngestionState,
    pub photos: Vec<ArchivedPhoto>,
    pub missing_sources: Vec<MissingSource>,
    pub notifications: u64,
}
pub(crate) struct Pending {
    pub(crate) owner: Option<GroupId>,
    pub(crate) task: Option<TaskId>,
    pub(crate) version: Option<archive::SourceFileVersion>,
    pub(crate) stable_since: Instant,
}
pub(crate) struct Ingestion {
    pub(crate) hot_folder: PathBuf,
    pub(crate) archive: PathBuf,
    pub(crate) pending: BTreeMap<PathBuf, Pending>,
    pub(crate) seen: BTreeMap<PathBuf, (archive::SourceFileVersion, Option<GroupId>)>,
    pub(crate) checked: BTreeMap<PathBuf, Instant>,
    pub(crate) last_audit: Instant,
    pub(crate) lost: BTreeSet<PathBuf>,
    pub(crate) photos: Vec<ArchivedPhoto>,
    pub(crate) missing_sources: Vec<MissingSource>,
    pub(crate) state: IngestionState,
    pub(crate) uncertain: bool,
}

/// Capture coordination lives in `capture_session`; this alias preserves the concise ingestion entry name.
pub use crate::capture_session::CaptureSession as HotFolder;

impl Ingestion {
    pub(crate) fn fail(&mut self, session: &mut Session, state: IngestionState) {
        if self.state != state {
            tracing::warn!(
                ?state,
                "Photo ingestion paused; group transitions are blocked"
            );
        }
        self.state = state;
        session.pause();
    }
    pub(crate) fn reconcile(&mut self, session: &mut Session) {
        match self.scan(session) {
            Ok(()) => {
                self.state = IngestionState::Running;
                session.resume();
                self.uncertain = false;
            }
            Err(error) => {
                let state = if matches!(
                    error,
                    Error::UnsafePath
                        | Error::Archive {
                            source: ArchiveError::UnsafePath
                        }
                ) {
                    IngestionState::PausedDirectory
                } else {
                    IngestionState::PausedArchive
                };
                tracing::warn!(error=%error,"Photo ingestion reconciliation failed");
                self.fail(session, state);
                self.uncertain = true;
            }
        }
    }
    pub(crate) fn scan(&mut self, session: &mut Session) -> Result<()> {
        self.scan_with_probe(session, archive::probe)
    }
    pub(crate) fn scan_with_probe(
        &mut self,
        session: &mut Session,
        mut probe: impl FnMut(&Path, &Path) -> archive::Result<archive::SourceFileVersion>,
    ) -> Result<()> {
        ensure!(
            fs::canonicalize(&self.hot_folder).context(IoSnafu)? == self.hot_folder,
            UnsafePathSnafu
        );
        let mut present = BTreeSet::new();
        for entry in fs::read_dir(&self.hot_folder).context(IoSnafu)? {
            let entry = entry.context(IoSnafu)?;
            let path = entry.path();
            let kind = entry.file_type().context(IoSnafu)?;
            ensure!(kind.is_file() && !kind.is_symlink(), UnsafePathSnafu);
            let supported = path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    ["jpg", "jpeg", "png"]
                        .iter()
                        .any(|ext| value.eq_ignore_ascii_case(ext))
                });
            if !supported {
                continue;
            }
            present.insert(path);
        }
        // Only a complete, valid directory listing can establish source loss.
        // Retire losses before observing new names, so renamed files need review.
        let missing: Vec<_> = self
            .pending
            .keys()
            .filter(|path| !present.contains(*path))
            .cloned()
            .collect();
        for path in missing {
            let pending = self.pending.get(&path).context(InvariantSnafu)?;
            if let (Some(group), Some(task)) = (pending.owner, pending.task) {
                session.task_finished(group, task).context(CaptureSnafu)?;
            }
            let pending = self.pending.remove(&path).context(InvariantSnafu)?;
            tracing::warn!("Photo source vanished before archival; photographer review required");
            self.lost.insert(path.clone());
            self.missing_sources
                .retain(|missing| missing.source != path);
            self.missing_sources.push(MissingSource {
                source: path,
                group_id: pending.owner,
            });
            self.uncertain = true;
        }
        // One full-file audit per 30 seconds globally, even with many originals.
        // Normal metadata changes bypass this low-priority content audit budget.
        let audit = (self.last_audit.elapsed() >= Duration::from_secs(30))
            .then(|| {
                self.checked
                    .iter()
                    .filter(|(path, _)| {
                        present.contains(*path) && !self.pending.contains_key(*path)
                    })
                    .min_by_key(|(_, at)| **at)
                    .filter(|(_, at)| at.elapsed() >= Duration::from_secs(30))
                    .map(|(path, _)| path.clone())
            })
            .flatten();
        if audit.is_some() {
            self.last_audit = Instant::now();
        }
        for path in present {
            if audit.as_ref() != Some(&path)
                && !self.pending.contains_key(&path)
                && let Some((version, _)) = self.seen.get(&path)
                && archive::unchanged(&path, version).context(ArchiveSnafu)?
            {
                continue;
            }
            self.observe(session, &path, &mut probe)?;
        }
        Ok(())
    }

    fn observe(
        &mut self,
        session: &mut Session,
        path: &Path,
        probe: &mut impl FnMut(&Path, &Path) -> archive::Result<archive::SourceFileVersion>,
    ) -> Result<()> {
        let version = match probe(path, &self.hot_folder) {
            Ok(version) => Some(version),
            Err(ArchiveError::Changed) => None,
            Err(ArchiveError::Io { source }) if matches!(source.raw_os_error(), Some(32 | 33)) => {
                None
            }
            Err(error) => return Err(ArchiveSnafu.into_error(error)),
        };
        if let Some(version) = &version
            && self.seen.get(path).is_some_and(|(seen, _)| seen == version)
        {
            self.checked.insert(path.to_owned(), Instant::now());
            if let Some(pending) = self.pending.remove(path)
                && let (Some(group), Some(task)) = (pending.owner, pending.task)
            {
                session.task_finished(group, task).context(CaptureSnafu)?;
            }
            return Ok(());
        }
        if !self.pending.contains_key(path) {
            let current = session.group().map(capture::Group::id);
            let crosses_group = self
                .seen
                .get(path)
                .is_some_and(|(_, owner)| *owner != current);
            let owner = if self.uncertain || crosses_group || self.lost.contains(path) {
                None
            } else {
                current
            };
            let task = owner
                .map(|id| session.task_started(id))
                .transpose()
                .context(CaptureSnafu)?;
            self.pending.insert(
                path.to_owned(),
                Pending {
                    owner,
                    task,
                    version: version.clone(),
                    stable_since: Instant::now(),
                },
            );
            return Ok(());
        }
        let pending = self.pending.get_mut(path).context(InvariantSnafu)?;
        if pending.version != version || version.is_none() {
            pending.version = version;
            pending.stable_since = Instant::now();
            return Ok(());
        }
        if pending.stable_since.elapsed() < INTERVAL {
            return Ok(());
        }
        let version = version.ok_or_else(|| ArchiveSnafu.into_error(ArchiveError::Changed))?;
        let destination = match archive::copy(path, &self.hot_folder, &self.archive, &version) {
            Ok(path) => path,
            Err(ArchiveError::Changed) => {
                pending.version = None;
                return Ok(());
            }
            Err(error) => return Err(ArchiveSnafu.into_error(error)),
        };
        let pending = self.pending.remove(path).context(InvariantSnafu)?;
        if let (Some(group), Some(task)) = (pending.owner, pending.task) {
            session.task_finished(group, task).context(CaptureSnafu)?;
        }
        self.seen
            .insert(path.to_owned(), (version.clone(), pending.owner));
        self.checked.insert(path.to_owned(), Instant::now());
        self.missing_sources
            .retain(|missing| missing.source != path);
        self.photos.push(ArchivedPhoto {
            id: Uuid::now_v7(),
            source: path.to_owned(),
            path: destination,
            sha256: version.sha256,
            bytes: version.bytes,
            group_id: pending.owner,
        });
        Ok(())
    }
}
