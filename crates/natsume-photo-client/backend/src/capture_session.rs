//! Owns capture policy and coordinates ordered ingestion work independently of UI.
pub use crate::hot_folder::archive::ArchiveError;
use crate::hot_folder::{archive, native::NativeMonitor};
use crate::{
    capture::{self, GroupId, Session},
    workspace::Workspace,
};
use snafu::{IntoError as _, OptionExt as _, ResultExt as _, Snafu, ensure};
use std::time::Duration;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
const INTERVAL: Duration = Duration::from_millis(250);
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

mod ingestion;
use ingestion::Ingestion;

struct CaptureTask {
    group: GroupId,
    id: capture::TaskId,
}

struct CaptureState {
    session: Session,
    ingestion: Ingestion,
    tasks: BTreeMap<PathBuf, CaptureTask>,
}
enum Action {
    Select(String),
    Start,
    Abandon(GroupId),
    Assign(Uuid, GroupId),
    AcknowledgeMissing(PathBuf),
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
pub struct CaptureSession {
    _workspace_owner: crate::workspace::WorkspaceWriter,
    commands: mpsc::Sender<Command>,
    snapshots: watch::Receiver<HotFolderSnapshot>,
    stop: CancellationToken,
    worker: Option<tokio::task::JoinHandle<Result<()>>>,
}
impl CaptureSession {
    /// Registers native monitoring before checking emptiness and starts reconciliation.
    /// # Errors
    /// Rejects nonempty, overlapping or unsafe directories and failed setup.
    pub async fn prepare(workspace: &Workspace, hot_folder: &Path) -> Result<Self> {
        let workspace_owner = workspace.writable_owner();
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
            let archive = root.join("captures").join(session.id().to_string());
            Ok(CaptureState {
                session,
                ingestion: Ingestion::new(hot_folder, archive),
                tasks: BTreeMap::new(),
            })
        })
        .await
        .context(WorkerSnafu)??;
        let (wake, events) = mpsc::channel(1);
        let (publish, snapshots) = watch::channel(inner.snapshot(0));
        let (commands, receive) = mpsc::channel(16);
        let stop = CancellationToken::new();
        let startup_guard = stop.clone().drop_guard();
        let (ready, started) = oneshot::channel();
        let worker_stop = stop.clone();
        let worker_owner = workspace_owner.clone();
        // WinRT query objects are apartment-bound and !Send. Construct, use and
        // release them on one Tokio-managed blocking thread; never assert Send.
        let worker = tokio::task::spawn_blocking(move || {
            // Drop only cancels: already-running I/O must retain ownership.
            let _owner = worker_owner;
            match tokio::runtime::Builder::new_current_thread()
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
                    StoppedSnafu.fail()
                }
            }
        });
        started.await.map_err(|_| StoppedSnafu.build())??;
        let _ = startup_guard.disarm();
        Ok(Self {
            _workspace_owner: workspace_owner,
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
    /// Dismisses an active loss warning. A returning source still needs review.
    /// # Errors
    /// Reports a stopped worker or an unknown warning.
    pub async fn acknowledge_missing(&self, source: &Path) -> Result<()> {
        self.request(Action::AcknowledgeMissing(source.to_owned()))
            .await
            .map(|_| ())
    }
    /// Stops accepting new work and waits for running blocking copies and cleanup.
    /// # Errors
    /// Reports a failed worker task.
    pub async fn shutdown(mut self) -> Result<()> {
        self.stop.cancel();
        if let Some(worker) = self.worker.take() {
            worker.await.context(WorkerSnafu)??;
        }
        Ok(())
    }
}
impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn prepare_archive(inner: CaptureState) -> Result<CaptureState> {
    tokio::task::spawn_blocking(move || {
        ensure!(
            fs::read_dir(inner.ingestion.hot_folder())
                .context(IoSnafu)?
                .next()
                .is_none(),
            NotEmptySnafu
        );
        fs::create_dir_all(inner.ingestion.archive()).context(IoSnafu)?;
        ensure!(
            fs::canonicalize(inner.ingestion.archive()).context(IoSnafu)?
                == inner.ingestion.archive(),
            UnsafePathSnafu
        );
        Ok(inner)
    })
    .await
    .context(WorkerSnafu)
    .and_then(|result| result)
}

async fn start_worker(
    inner: CaptureState,
    wake: mpsc::Sender<()>,
    ready: oneshot::Sender<Result<()>>,
) -> Result<(CaptureState, NativeMonitor)> {
    let monitor = match NativeMonitor::open(inner.ingestion.hot_folder(), wake.clone())
        .await
        .context(MonitorSnafu)
    {
        Ok(monitor) => monitor,
        Err(error) => {
            let _ = ready.send(Err(error));
            return StoppedSnafu.fail();
        }
    };
    let initialized = prepare_archive(inner).await;
    let inner = match initialized {
        Ok(inner) => inner,
        Err(error) => {
            let _ = ready.send(Err(error));
            return StoppedSnafu.fail();
        }
    };
    let _ = ready.send(Ok(()));
    Ok((inner, monitor))
}

async fn run_worker(
    inner: CaptureState,
    mut commands: mpsc::Receiver<Command>,
    mut events: mpsc::Receiver<()>,
    wake: mpsc::Sender<()>,
    publish: watch::Sender<HotFolderSnapshot>,
    stop: CancellationToken,
    ready: oneshot::Sender<Result<()>>,
) -> Result<()> {
    let (mut inner, monitor) = start_worker(inner, wake.clone(), ready).await?;
    let mut monitor = Some(monitor);
    let mut notifications = 0;
    let mut stamp = fs::metadata(inner.ingestion.hot_folder())
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
        let folder = inner.ingestion.hot_folder().to_owned();
        let current = tokio::task::spawn_blocking(move || {
            fs::metadata(folder).ok().and_then(|m| m.created().ok())
        })
        .await;
        let current = match current {
            Ok(current) => current,
            Err(source) => {
                let mut stopped = publish.borrow().clone();
                stopped.state = IngestionState::Stopped;
                stopped.paused = true;
                publish.send_replace(stopped);
                return Err(WorkerSnafu.into_error(source));
            }
        };
        if current != stamp || current.is_none() {
            monitor = None;
            inner.monitor_lost();
            stamp = current;
        }
        if monitor.is_none() {
            inner.monitor_lost();
            if let Ok(replacement) =
                NativeMonitor::open(inner.ingestion.hot_folder(), wake.clone()).await
            {
                monitor = Some(replacement);
            }
        }
        let available = monitor.is_some();
        let worked = tokio::task::spawn_blocking(move || {
            let reply = if let Some(command) = command {
                let result = inner.apply(command.action, available);
                Some((command.reply, result))
            } else if available {
                let _ = inner.apply(Action::Scan, true);
                None
            } else {
                None
            };
            (inner, reply)
        })
        .await;
        let (returned, reply) = match worked {
            Ok(result) => result,
            Err(source) => {
                tracing::error!("Photo ingestion blocking task failed");
                let mut stopped = publish.borrow().clone();
                stopped.state = IngestionState::Stopped;
                stopped.paused = true;
                publish.send_replace(stopped);
                return Err(WorkerSnafu.into_error(source));
            }
        };
        inner = returned;
        publish.send_replace(inner.snapshot(notifications));
        if let Some((reply, result)) = reply {
            let _ = reply.send(result);
        }
    }
    drop(monitor);
    inner.ingestion.stop();
    inner.session.pause();
    publish.send_replace(inner.snapshot(notifications));
    Ok(())
}
impl CaptureState {
    fn monitor_lost(&mut self) {
        self.ingestion.monitor_lost();
        self.session.pause();
    }
    fn sync_tasks(&mut self) -> Result<()> {
        let pending: BTreeMap<_, _> = self
            .ingestion
            .pending_owners()
            .map(|(path, group)| (path.clone(), group))
            .collect();
        let finished: Vec<_> = self
            .tasks
            .keys()
            .filter(|path| !pending.contains_key(*path))
            .cloned()
            .collect();
        for path in finished {
            let task = self.tasks.remove(&path).context(InvariantSnafu)?;
            self.session
                .task_finished(task.group, task.id)
                .context(CaptureSnafu)?;
        }
        for (path, group) in pending {
            if let std::collections::btree_map::Entry::Vacant(entry) = self.tasks.entry(path) {
                let task = self.session.task_started(group).context(CaptureSnafu)?;
                entry.insert(CaptureTask { group, id: task });
            }
        }
        Ok(())
    }
    #[cfg(test)]
    fn scan(&mut self) -> Result<()> {
        let result = self
            .ingestion
            .scan(self.session.group().map(capture::Group::id));
        self.sync_tasks()?;
        result
    }
    fn reconcile(&mut self) {
        let result = self
            .ingestion
            .reconcile(self.session.group().map(capture::Group::id));
        let result = result.and(self.sync_tasks());
        match result {
            Ok(()) => self.session.resume(),
            Err(error) => {
                tracing::warn!(error = %error, "Photo ingestion reconciliation failed");
                self.session.pause();
            }
        }
    }

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
            state: self.ingestion.state(),
            photos: self.ingestion.photos().to_vec(),
            missing_sources: self.ingestion.missing_sources().to_vec(),
            notifications,
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
                self.ingestion.assign_reviewed(photo, group)?;
            }
            Action::Scan => self.reconcile(),
            Action::AcknowledgeMissing(path) => {
                self.ingestion.acknowledge_missing(&path)?;
            }
        }
        Ok(Response::Done)
    }
}

#[cfg(test)]
#[path = "../tests/unit/hot_folder_session.rs"]
mod integration_tests;
