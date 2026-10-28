//! Tokio-owned ingestion state, safe native notifications and copy-only archives.
use crate::{
    capture::{self, GroupId, Session, TaskId},
    workspace::Workspace,
};
use snafu::{IntoError as _, OptionExt as _, ResultExt as _, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod archive;
mod native;
pub use archive::ArchiveError;
use native::NativeMonitor;

const INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Snafu)]
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
    pub notifications: u64,
}
struct Pending {
    owner: Option<GroupId>,
    task: Option<TaskId>,
    version: Option<archive::SourceFileVersion>,
    stable_since: Instant,
}
struct Inner {
    session: Session,
    hot_folder: PathBuf,
    archive: PathBuf,
    pending: BTreeMap<PathBuf, Pending>,
    seen: BTreeMap<PathBuf, (archive::SourceFileVersion, Option<GroupId>)>,
    photos: Vec<ArchivedPhoto>,
    state: IngestionState,
    uncertain: bool,
}
enum Action {
    Select(String),
    Start,
    Abandon(GroupId),
    Assign(Uuid, GroupId),
    Scan,
}
enum Response {
    Done,
    Group(GroupId),
}
struct Command {
    action: Action,
    reply: oneshot::Sender<Result<Response>>,
}

/// Owns a Tokio worker; explicit shutdown drains running I/O, Drop only requests it.
pub struct HotFolder {
    commands: mpsc::Sender<Command>,
    snapshots: watch::Receiver<HotFolderSnapshot>,
    stop: CancellationToken,
    worker: Option<tokio::task::JoinHandle<()>>,
}
impl HotFolder {
    /// Registers native monitoring before checking emptiness and starts reconciliation.
    /// # Errors
    /// Rejects nonempty, overlapping or unsafe directories and failed setup.
    pub async fn prepare(workspace: &Workspace, hot_folder: &Path) -> Result<Self> {
        let session = Session::new(workspace.roster().await.context(WorkspaceSnafu)?);
        let hot_folder = hot_folder.to_owned();
        let root = workspace.root().to_owned();
        let inner = tokio::task::spawn_blocking(move || {
            ensure!(
                hot_folder.is_absolute()
                    && !hot_folder.components().any(|c| c == Component::ParentDir),
                UnsafePathSnafu
            );
            let hot_folder = fs::canonicalize(hot_folder).context(IoSnafu)?;
            ensure!(
                hot_folder.is_dir()
                    && !hot_folder.starts_with(&root)
                    && !root.starts_with(&hot_folder),
                UnsafePathSnafu
            );
            Ok(Inner {
                archive: root.join("captures").join(session.id().to_string()),
                session,
                hot_folder,
                pending: BTreeMap::new(),
                seen: BTreeMap::new(),
                photos: Vec::new(),
                state: IngestionState::Running,
                uncertain: false,
            })
        })
        .await
        .context(WorkerSnafu)??;
        let (wake, events) = mpsc::channel(1);
        let (publish, snapshots) = watch::channel(inner.snapshot(0));
        let (commands, receive) = mpsc::channel(16);
        let stop = CancellationToken::new();
        let (ready, started) = oneshot::channel();
        let worker_stop = stop.clone();
        // WinRT query objects are apartment-bound and !Send. Construct, use and
        // release them on one Tokio-managed blocking thread; never assert Send.
        let worker =
            tokio::task::spawn_blocking(
                move || match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(run_worker(
                        inner,
                        receive,
                        events,
                        wake,
                        publish,
                        worker_stop,
                        ready,
                    )),
                    Err(source) => {
                        let _ = ready.send(Err(IoSnafu.into_error(source)));
                    }
                },
            );
        started.await.map_err(|_| StoppedSnafu.build())??;
        Ok(Self {
            commands,
            snapshots,
            stop,
            worker: Some(worker),
        })
    }
    /// Returns the most recently published facts without scanning or blocking I/O.
    /// # Errors
    /// Reserved for ingestion API consistency; stopped state is observable in the result.
    pub fn snapshot(&self) -> Result<HotFolderSnapshot> {
        Ok(self.snapshots.borrow().clone())
    }
    /// Waits for published facts only; it does not drive ingestion or refresh files.
    /// # Errors
    /// Returns Stopped if the worker exits before the predicate is satisfied.
    pub async fn wait_until(
        &self,
        timeout: Duration,
        predicate: impl Fn(&HotFolderSnapshot) -> bool,
    ) -> Result<Option<HotFolderSnapshot>> {
        let mut snapshots = self.snapshots.clone();
        match tokio::time::timeout(timeout, async {
            loop {
                let snapshot = snapshots.borrow_and_update().clone();
                if predicate(&snapshot) {
                    return Ok(snapshot);
                }
                snapshots
                    .changed()
                    .await
                    .map_err(|_| StoppedSnafu.build())?;
            }
        })
        .await
        {
            Ok(result) => result.map(Some),
            Err(_) => Ok(None),
        }
    }
    async fn request(&self, action: Action) -> Result<Response> {
        let (reply, receive) = oneshot::channel();
        self.commands
            .send(Command { action, reply })
            .await
            .map_err(|_| StoppedSnafu.build())?;
        receive.await.map_err(|_| StoppedSnafu.build())?
    }
    /// Selects a known team without starting a group.
    /// # Errors
    /// Rejects active/paused groups and a stopped worker.
    pub async fn select_team(&self, account_id: &str) -> Result<()> {
        self.request(Action::Select(account_id.to_owned()))
            .await
            .map(|_| ())
    }
    /// Reconciles existing files before locking the selected team into a new group.
    /// # Errors
    /// Rejects unavailable monitoring, paused state and invalid group transitions.
    pub async fn start_group(&self) -> Result<GroupId> {
        match self.request(Action::Start).await? {
            Response::Group(id) => Ok(id),
            Response::Done => InvariantSnafu.fail(),
        }
    }
    /// Explicitly abandons an unconfirmed group without deleting its source images.
    /// # Errors
    /// Rejects pending work, stale identities and unavailable monitoring.
    pub async fn abandon_group(&self, id: GroupId) -> Result<()> {
        self.request(Action::Abandon(id)).await.map(|_| ())
    }
    /// Assigns an unowned archive after photographer review.
    /// # Errors
    /// Rejects paused groups and invalid or already-owned archives.
    pub async fn assign_reviewed(&self, photo: Uuid, group: GroupId) -> Result<()> {
        self.request(Action::Assign(photo, group)).await.map(|_| ())
    }
    /// Stops accepting new work and waits for running blocking copies and cleanup.
    /// # Errors
    /// Reports a failed worker task.
    pub async fn shutdown(mut self) -> Result<()> {
        self.stop.cancel();
        if let Some(worker) = self.worker.take() {
            worker.await.context(WorkerSnafu)?;
        }
        Ok(())
    }
}
impl Drop for HotFolder {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn prepare_archive(inner: Inner) -> Result<Inner> {
    tokio::task::spawn_blocking(move || {
        ensure!(
            fs::read_dir(&inner.hot_folder)
                .context(IoSnafu)?
                .next()
                .is_none(),
            NotEmptySnafu
        );
        fs::create_dir_all(&inner.archive).context(IoSnafu)?;
        ensure!(
            fs::canonicalize(&inner.archive).context(IoSnafu)? == inner.archive,
            UnsafePathSnafu
        );
        Ok(inner)
    })
    .await
    .context(WorkerSnafu)
    .and_then(|result| result)
}

async fn run_worker(
    inner: Inner,
    mut commands: mpsc::Receiver<Command>,
    mut events: mpsc::Receiver<()>,
    wake: mpsc::Sender<()>,
    publish: watch::Sender<HotFolderSnapshot>,
    stop: CancellationToken,
    ready: oneshot::Sender<Result<()>>,
) {
    let monitor = match NativeMonitor::open(&inner.hot_folder, wake.clone())
        .await
        .context(MonitorSnafu)
    {
        Ok(monitor) => monitor,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let initialized = prepare_archive(inner).await;
    let mut inner = match initialized {
        Ok(inner) => inner,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let mut monitor = Some(monitor);
    let mut notifications = 0;
    let mut stamp = fs::metadata(&inner.hot_folder)
        .ok()
        .and_then(|m| m.created().ok());
    let mut timer = tokio::time::interval(INTERVAL);
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let command = tokio::select! {
            biased;
            ()=stop.cancelled()=>break,
            event=events.recv()=>{
                if event.is_none(){break;}
                notifications+=1;
                if let Some(current)=&monitor && current.refresh().await.is_err(){monitor=None;}
                None
            },
            command=commands.recv()=>{if command.is_none(){break;}command},
            _=timer.tick()=>None,
        };
        let folder = inner.hot_folder.clone();
        let current = tokio::task::spawn_blocking(move || {
            fs::metadata(folder).ok().and_then(|m| m.created().ok())
        })
        .await;
        let Ok(current) = current else {
            break;
        };
        if current != stamp || current.is_none() {
            monitor = None;
            inner.uncertain = true;
            inner.fail(IngestionState::RecoveringMonitor);
            stamp = current;
        }
        if monitor.is_none() {
            inner.fail(IngestionState::RecoveringMonitor);
            if let Ok(replacement) = NativeMonitor::open(&inner.hot_folder, wake.clone()).await {
                monitor = Some(replacement);
            }
        }
        let available = monitor.is_some();
        let worked = tokio::task::spawn_blocking(move || {
            if let Some(command) = command {
                let result = inner.apply(command.action, available);
                let _ = command.reply.send(result);
            } else if available {
                let _ = inner.apply(Action::Scan, true);
            }
            inner
        })
        .await;
        let Ok(returned) = worked else {
            tracing::error!("Photo ingestion blocking task failed");
            let mut stopped = publish.borrow().clone();
            stopped.state = IngestionState::Stopped;
            stopped.paused = true;
            publish.send_replace(stopped);
            return;
        };
        inner = returned;
        publish.send_replace(inner.snapshot(notifications));
    }
    drop(monitor);
    inner.state = IngestionState::Stopped;
    inner.session.pause();
    publish.send_replace(inner.snapshot(notifications));
}
impl Inner {
    fn snapshot(&self, notifications: u64) -> HotFolderSnapshot {
        HotFolderSnapshot {
            session_id: self.session.id(),
            group_id: self.session.group().map(capture::Group::id),
            account_id: self.session.group().map(|g| g.account_id().to_owned()),
            pending_tasks: self
                .session
                .group()
                .map_or(0, capture::Group::pending_tasks),
            paused: self.session.is_paused(),
            state: self.state,
            photos: self.photos.clone(),
            notifications,
        }
    }
    fn fail(&mut self, state: IngestionState) {
        if self.state != state {
            tracing::warn!(
                ?state,
                "Photo ingestion paused; group transitions are blocked"
            );
        }
        self.state = state;
        self.session.pause();
    }
    fn reconcile(&mut self) {
        match self.scan() {
            Ok(()) => {
                self.state = IngestionState::Running;
                self.session.resume();
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
                self.fail(state);
                self.uncertain = true;
            }
        }
    }
    fn apply(&mut self, action: Action, monitor_available: bool) -> Result<Response> {
        match action {
            Action::Select(account) => {
                self.session.select_team(&account).context(CaptureSnafu)?;
            }
            Action::Start => {
                ensure!(monitor_available, MonitorUnavailableSnafu);
                self.reconcile();
                return self
                    .session
                    .start_group()
                    .map(Response::Group)
                    .context(CaptureSnafu);
            }
            Action::Abandon(id) => {
                ensure!(monitor_available, MonitorUnavailableSnafu);
                self.reconcile();
                self.session.abandon_group(id).context(CaptureSnafu)?;
            }
            Action::Assign(photo, group) => {
                ensure!(
                    !self.session.is_paused()
                        && self.session.group().is_some_and(|g| g.id() == group),
                    NotReviewableSnafu
                );
                self.photos
                    .iter_mut()
                    .find(|p| p.id == photo && p.group_id.is_none())
                    .context(NotReviewableSnafu)?
                    .group_id = Some(group);
            }
            Action::Scan => self.reconcile(),
        }
        Ok(Response::Done)
    }
    fn scan(&mut self) -> Result<()> {
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
            present.insert(path.clone());
            self.observe(&path)?;
        }
        // Disappearing pending sources are never silently declared completed.
        ensure!(
            self.pending.keys().all(|path| present.contains(path)),
            InvariantSnafu
        );
        Ok(())
    }

    fn observe(&mut self, path: &Path) -> Result<()> {
        let version = match archive::probe(path, &self.hot_folder) {
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
            if let Some(pending) = self.pending.remove(path)
                && let (Some(group), Some(task)) = (pending.owner, pending.task)
            {
                self.session
                    .task_finished(group, task)
                    .context(CaptureSnafu)?;
            }
            return Ok(());
        }
        if !self.pending.contains_key(path) {
            let current = self.session.group().map(capture::Group::id);
            let crosses_group = self
                .seen
                .get(path)
                .is_some_and(|(_, owner)| *owner != current);
            let owner = if self.uncertain || crosses_group {
                None
            } else {
                current
            };
            let task = owner
                .map(|id| self.session.task_started(id))
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
            self.session
                .task_finished(group, task)
                .context(CaptureSnafu)?;
        }
        self.seen
            .insert(path.to_owned(), (version.clone(), pending.owner));
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

#[cfg(test)]
mod tests;
