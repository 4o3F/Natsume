//! Automatic in-process monitoring and reconciliation, with copy-only archives.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use snafu::{IntoError as _, OptionExt as _, ResultExt as _, Snafu, ensure};
use uuid::Uuid;

use crate::{
    capture::{self, GroupId, Session, TaskId},
    workspace::Workspace,
};

mod archive;

const INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("热文件夹或归档文件操作失败"))]
    Io { source: std::io::Error },
    #[snafu(display("无法建立热文件夹监控"))]
    Watch { source: notify::Error },
    #[snafu(display("无法读取工作区名单"))]
    Workspace { source: crate::workspace::Error },
    #[snafu(display("拍摄组操作失败：{source}"))]
    Capture { source: capture::Error },
    #[snafu(display("请选择与工作区不重叠的绝对目录，且不能包含链接越界"))]
    UnsafePath,
    #[snafu(display("新会话要求热文件夹完全为空，请自行整理或选择新的空目录"))]
    NotEmpty,
    #[snafu(display("源文件仍在变化，请等待后重试"))]
    Changed,
    #[snafu(display("照片源文件超过 50 MiB 限额"))]
    Limit,
    #[snafu(display("热文件夹后台状态不可用"))]
    Worker,
    #[snafu(display("监控尚未恢复，禁止开始或关闭拍摄组"))]
    MonitorUnavailable,
    #[snafu(display("已有归档内容损坏，禁止覆盖，请先检查本地文件"))]
    InvalidArchive,
    #[snafu(display("指定的待核对归档不存在或已有队伍归属"))]
    NotReviewable,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Issue {
    Monitor,
    Directory,
    Archive,
}

/// An immutable source copy, not a decoded candidate or confirmed final.
#[derive(Debug, Clone)]
pub struct ArchivedPhoto {
    pub id: Uuid,
    pub source: PathBuf,
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    /// None means explicit photographer review is required.
    pub group_id: Option<GroupId>,
}

/// Owned facts for a future operator UI, never the read-only contestant screen.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub session_id: Uuid,
    pub group_id: Option<GroupId>,
    pub account_id: Option<String>,
    pub pending_tasks: usize,
    pub paused: bool,
    pub issue: Option<Issue>,
    pub photos: Vec<ArchivedPhoto>,
    /// Native event count helps distinguish watcher traffic from reconciliation.
    pub notifications: u64,
}

struct Pending {
    owner: Option<GroupId>,
    task: Option<TaskId>,
    version: Option<archive::Version>,
    stable_since: Instant,
}

struct Inner {
    session: Session,
    hot_folder: PathBuf,
    archive: PathBuf,
    pending: BTreeMap<PathBuf, Pending>,
    seen: BTreeMap<PathBuf, (archive::Version, Option<GroupId>)>,
    photos: Vec<ArchivedPhoto>,
    issue: Option<Issue>,
    uncertain: bool,
}

struct Shared {
    inner: Mutex<Inner>,
    changed: Condvar,
    stopped: AtomicBool,
    monitor_fault: AtomicBool,
    notifications: AtomicU64,
}

/// Owns the background worker. Drop stops and joins it; no state is serialized.
pub struct HotFolder {
    shared: Arc<Shared>,
    wake: SyncSender<()>,
    worker: Option<JoinHandle<()>>,
}

impl HotFolder {
    /// Registers native monitoring before testing emptiness, then starts automatic
    /// reconciliation. Does not perform display or source-enrollment admission.
    ///
    /// # Errors
    /// Rejects nonempty/overlapping/invalid paths or failed watcher/workspace setup.
    pub fn prepare(workspace: &mut Workspace, hot_folder: &Path) -> Result<Self> {
        ensure!(
            hot_folder.is_absolute()
                && !hot_folder
                    .components()
                    .any(|part| part == Component::ParentDir),
            UnsafePathSnafu
        );
        let hot_folder = fs::canonicalize(hot_folder).context(IoSnafu)?;
        ensure!(hot_folder.is_dir(), UnsafePathSnafu);
        ensure!(
            !hot_folder.starts_with(workspace.root()) && !workspace.root().starts_with(&hot_folder),
            UnsafePathSnafu
        );
        let session = Session::new(workspace.roster().context(WorkspaceSnafu)?);
        let (wake, receiver) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                archive: workspace
                    .root()
                    .join("captures")
                    .join(session.id().to_string()),
                session,
                hot_folder: hot_folder.clone(),
                pending: BTreeMap::new(),
                seen: BTreeMap::new(),
                photos: Vec::new(),
                issue: None,
                uncertain: false,
            }),
            changed: Condvar::new(),
            stopped: AtomicBool::new(false),
            monitor_fault: AtomicBool::new(false),
            notifications: AtomicU64::new(0),
        });
        let watcher = watch(&hot_folder, &shared, &wake)?;
        ensure!(
            fs::read_dir(&hot_folder).context(IoSnafu)?.next().is_none(),
            NotEmptySnafu
        );
        let created = fs::metadata(&hot_folder).context(IoSnafu)?.created().ok();
        {
            let inner = shared.inner.lock().map_err(|_| Error::Worker)?;
            let parent = inner.archive.parent().context(UnsafePathSnafu)?;
            match fs::create_dir(parent) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(source) => return Err(IoSnafu.into_error(source)),
            }
            // Validate the existing parent before creating anything beneath it.
            ensure!(
                fs::canonicalize(parent).context(IoSnafu)? == parent,
                UnsafePathSnafu
            );
            fs::create_dir(&inner.archive).context(IoSnafu)?;
            ensure!(
                fs::canonicalize(&inner.archive).context(IoSnafu)? == inner.archive,
                UnsafePathSnafu
            );
        }
        let worker_shared = Arc::clone(&shared);
        let worker_wake = wake.clone();
        let worker = thread::Builder::new()
            .name("photo-hot-folder".into())
            .spawn(move || {
                run_worker(&worker_shared, &receiver, &worker_wake, watcher, created);
            })
            .context(IoSnafu)?;
        Ok(Self {
            shared,
            wake,
            worker: Some(worker),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Inner>> {
        self.shared.inner.lock().map_err(|_| Error::Worker)
    }

    /// # Errors
    /// Returns a worker error if its state cannot be accessed.
    pub fn snapshot(&self) -> Result<Snapshot> {
        Ok(self
            .lock()?
            .snapshot(self.shared.notifications.load(Ordering::Relaxed)))
    }

    /// Waits for observable state, with a deadline. The background worker still
    /// performs all monitoring and reconciliation; this is not manual refresh.
    ///
    /// # Errors
    /// Returns a worker error if locking/waiting fails.
    pub fn wait_until(
        &self,
        timeout: Duration,
        predicate: impl Fn(&Snapshot) -> bool,
    ) -> Result<Option<Snapshot>> {
        let deadline = Instant::now() + timeout;
        let mut inner = self.lock()?;
        loop {
            let snapshot = inner.snapshot(self.shared.notifications.load(Ordering::Relaxed));
            if predicate(&snapshot) {
                return Ok(Some(snapshot));
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Ok(None);
            };
            let (guard, _) = self
                .shared
                .changed
                .wait_timeout(inner, remaining)
                .map_err(|_| Error::Worker)?;
            inner = guard;
        }
    }

    /// # Errors
    /// Rejects worker failure or invalid selection according to capture rules.
    pub fn select_team(&self, account_id: &str) -> Result<()> {
        self.lock()?
            .session
            .select_team(account_id)
            .context(CaptureSnafu)
    }

    /// Reconciles before the boundary, so already-present files are never first
    /// observed under the new group. This cannot prove the camera has no late files.
    ///
    /// # Errors
    /// Rejects paused/invalid capture state or worker failure.
    pub fn start_group(&self) -> Result<GroupId> {
        let mut inner = self.lock()?;
        ensure!(
            inner.issue != Some(Issue::Monitor)
                && !self.shared.monitor_fault.load(Ordering::Acquire),
            MonitorUnavailableSnafu
        );
        inner.reconcile();
        inner.session.start_group().context(CaptureSnafu)
    }

    /// # Errors
    /// Rejects paused groups, pending tasks, or mismatched group identities.
    pub fn abandon_group(&self, group_id: GroupId) -> Result<()> {
        let mut inner = self.lock()?;
        ensure!(
            inner.issue != Some(Issue::Monitor)
                && !self.shared.monitor_fault.load(Ordering::Acquire),
            MonitorUnavailableSnafu
        );
        inner.reconcile();
        inner.session.abandon_group(group_id).context(CaptureSnafu)
    }

    /// Explicitly assigns an unowned archive to the active group after review.
    ///
    /// # Errors
    /// Rejects absent/paused/mismatched groups or photos already assigned.
    pub fn assign_reviewed(&self, photo_id: Uuid, group_id: GroupId) -> Result<()> {
        let mut inner = self.lock()?;
        ensure!(!inner.session.is_paused(), NotReviewableSnafu);
        let group = inner.session.group().context(NotReviewableSnafu)?;
        ensure!(group.id() == group_id, NotReviewableSnafu);
        let photo = inner
            .photos
            .iter_mut()
            .find(|photo| photo.id == photo_id && photo.group_id.is_none())
            .context(NotReviewableSnafu)?;
        photo.group_id = Some(group_id);
        Ok(())
    }
}

impl Drop for HotFolder {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!("热文件夹后台线程异常退出");
        }
    }
}

fn watch(path: &Path, shared: &Arc<Shared>, wake: &SyncSender<()>) -> Result<RecommendedWatcher> {
    let shared = Arc::clone(shared);
    let wake = wake.clone();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        if let Ok(event) = &result {
            // Reading/hashing a file must not create a self-sustaining event loop.
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
        }
        shared.notifications.fetch_add(1, Ordering::Relaxed);
        if !result.as_ref().is_ok_and(|event| !event.need_rescan()) {
            shared.monitor_fault.store(true, Ordering::Release);
        }
        // Bounded notifications are hints. A full channel is covered by scanning.
        let _ = wake.try_send(());
    })
    .context(WatchSnafu)?;
    watcher
        .watch(path, RecursiveMode::NonRecursive)
        .context(WatchSnafu)?;
    Ok(watcher)
}

fn run_worker(
    shared: &Arc<Shared>,
    receiver: &mpsc::Receiver<()>,
    wake: &SyncSender<()>,
    watcher: RecommendedWatcher,
    mut created: Option<SystemTime>,
) {
    let mut watcher = Some(watcher);
    while !shared.stopped.load(Ordering::Acquire) {
        let _ = receiver.recv_timeout(INTERVAL);
        if shared.stopped.load(Ordering::Acquire) {
            break;
        }
        let Ok(mut inner) = shared.inner.lock() else {
            tracing::error!("热文件夹后台锁失效");
            break;
        };
        let stamp = fs::metadata(&inner.hot_folder)
            .ok()
            .and_then(|meta| meta.created().ok());
        if shared.monitor_fault.swap(false, Ordering::AcqRel) || stamp != created {
            watcher = None;
            inner.uncertain = true;
            inner.fail(Issue::Monitor);
            created = stamp;
            shared.changed.notify_all();
            continue;
        }
        if watcher.is_none() {
            if let Ok(replacement) = watch(&inner.hot_folder, shared, wake) {
                watcher = Some(replacement);
            } else {
                inner.fail(Issue::Monitor);
                shared.changed.notify_all();
                continue;
            }
        }
        inner.reconcile();
        shared.changed.notify_all();
    }
}

impl Inner {
    fn snapshot(&self, notifications: u64) -> Snapshot {
        Snapshot {
            session_id: self.session.id(),
            group_id: self.session.group().map(capture::Group::id),
            account_id: self
                .session
                .group()
                .map(|group| group.account_id().to_owned()),
            pending_tasks: self
                .session
                .group()
                .map_or(0, capture::Group::pending_tasks),
            paused: self.session.is_paused(),
            issue: self.issue,
            photos: self.photos.clone(),
            notifications,
        }
    }

    fn fail(&mut self, issue: Issue) {
        if self.issue != Some(issue) {
            tracing::warn!(?issue, "收图暂停，等待自动重试；禁止切组");
        }
        self.issue = Some(issue);
        self.session.pause();
    }

    fn reconcile(&mut self) {
        match self.scan() {
            Ok(()) => {
                self.issue = None;
                self.session.resume();
                self.uncertain = false;
            }
            Err(error) => {
                // Error sources stay typed here, never log paths or image bytes.
                let issue = if matches!(error, Error::UnsafePath | Error::NotEmpty) {
                    Issue::Directory
                } else {
                    Issue::Archive
                };
                self.fail(issue);
                self.uncertain = true;
            }
        }
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
            ChangedSnafu
        );
        Ok(())
    }

    fn observe(&mut self, path: &Path) -> Result<()> {
        let version = match archive::probe(path, &self.hot_folder) {
            Ok(version) => Some(version),
            Err(Error::Changed) => None,
            Err(Error::Io { source }) if matches!(source.raw_os_error(), Some(32 | 33)) => None,
            Err(error) => return Err(error),
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
        let pending = self.pending.get_mut(path).context(WorkerSnafu)?;
        if pending.version != version || version.is_none() {
            pending.version = version;
            pending.stable_since = Instant::now();
            return Ok(());
        }
        if pending.stable_since.elapsed() < INTERVAL {
            return Ok(());
        }
        let version = version.context(ChangedSnafu)?;
        let destination = match archive::copy(path, &self.hot_folder, &self.archive, &version) {
            Ok(path) => path,
            Err(Error::Changed) => {
                pending.version = None;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let pending = self.pending.remove(path).context(WorkerSnafu)?;
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
