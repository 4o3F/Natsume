use std::{
    fs,
    path::{Path, PathBuf},
};

use snafu::{ResultExt as _, Snafu};
use tokio::task::JoinHandle;

use natsume_workstation_ipc::{
    GraphicalSession, GraphicalSessionState, HomeResetPhase, HomeResetProgress,
    ResourceControlError, SessionForeground, SessionRole,
};
use zbus::Connection;

use super::{rejected, unavailable};
use crate::{admission, session};

#[derive(Default)]
pub(crate) struct RecoveryObservation {
    pub(crate) action: Option<RecoveryAction>,
    pub(crate) reset_epoch: Option<u64>,
}

#[derive(Clone, Copy)]
pub(crate) enum RecoveryAction {
    Resume,
    Baseline,
}
impl RecoveryAction {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Resume => "resume",
            Self::Baseline => "baseline",
        }
    }
}

pub(crate) struct ResetTask {
    epoch: u64,
    handle: JoinHandle<Result<(), ResetError>>,
}

impl ResetTask {
    pub(crate) fn progress(&self) -> Option<HomeResetProgress> {
        (!self.handle.is_finished()).then_some(HomeResetProgress {
            reset_epoch: self.epoch,
            phase: HomeResetPhase::InProgress,
        })
    }
}

#[derive(Debug, Snafu)]
enum ResetError {
    #[snafu(display("Home reset background operation failed"))]
    Operation { source: ResourceControlError },
    #[snafu(display("Home reset background task stopped"))]
    Join { source: tokio::task::JoinError },
    #[snafu(display("Home reset completion could not be verified"))]
    Incomplete,
}

impl ResetError {
    fn category(&self) -> &'static str {
        match self {
            Self::Operation {
                source: ResourceControlError::Rejected(_),
            } => "rejected",
            Self::Operation { .. } => "unavailable",
            Self::Join { .. } => "task_stopped",
            Self::Incomplete => "incomplete",
        }
    }

    fn into_control(self) -> ResourceControlError {
        match self {
            Self::Operation { source } => source,
            Self::Join { .. } | Self::Incomplete => {
                unavailable("Home reset background work is incomplete")
            }
        }
    }
}

async fn task_running(
    task: &mut Option<ResetTask>,
    epoch: u64,
) -> Result<bool, ResourceControlError> {
    let Some(current) = task.as_ref() else {
        return Ok(false);
    };
    if !current.handle.is_finished() {
        if current.epoch != epoch {
            return Err(rejected("another Home reset epoch is in progress"));
        }
        return Ok(true);
    }
    if let Some(finished) = task.take() {
        finished
            .handle
            .await
            .context(JoinSnafu)
            .inspect_err(|error| {
                tracing::warn!(
                    reset_epoch = finished.epoch,
                    phase = "join",
                    error_category = error.category(),
                    "home reset background task unavailable"
                );
            })
            .and_then(|result| result)
            .map_err(ResetError::into_control)?;
    }
    Ok(false)
}

fn start_task(
    root: &Path,
    epoch: u64,
    mutation: admission::MutationGuard,
    guard: admission::MaintenanceGuard,
    remove: impl FnOnce(&Path, u64) -> Result<(), ResourceControlError> + Send + 'static,
) -> Result<ResetTask, ResourceControlError> {
    super::write_progress(root, epoch, HomeResetPhase::InProgress)?;
    let root: PathBuf = root.to_owned();
    let handle = tokio::task::spawn_blocking(move || {
        let _mutation = mutation;
        let started = std::time::Instant::now();
        let mut phase = "remove_generations";
        let result = (|| {
            remove(&root, epoch).context(OperationSnafu)?;
            phase = "verify";
            super::cleanup::require_policy(&root).context(OperationSnafu)?;
            snafu::ensure!(
                super::cleanup::complete(&root, epoch).context(OperationSnafu)?
                    && super::mounted_generation(&root, epoch).context(OperationSnafu)?,
                IncompleteSnafu
            );
            let verified = super::verify(&root, epoch).context(OperationSnafu)?;
            snafu::ensure!(verified.phase == HomeResetPhase::Verified, IncompleteSnafu);
            phase = "publish_permit";
            drop(guard);
            finish(&root, epoch).context(OperationSnafu)
        })();
        tracing::info!(
            reset_epoch = epoch,
            phase,
            elapsed_us = crate::duration_micros(started.elapsed()),
            result = if result.is_ok() { "ok" } else { "error" },
            error_category = result.as_ref().err().map(ResetError::category),
            "home reset background completed"
        );
        result
    });
    Ok(ResetTask { epoch, handle })
}

const WINDOW: &str = "login-window";

/// The captured session is immutable across retries, Helper restarts and boots.
/// A window itself closes PAM admission, including a crash before permit removal.
#[derive(Debug, PartialEq, Eq)]
struct Window {
    epoch: u64,
    boot: String,
    contest_id: Option<String>,
    maintaining: bool,
}

impl Window {
    fn read(root: &Path) -> Result<Option<Self>, ResourceControlError> {
        let encoded = match fs::read_to_string(super::state_directory(root).join(WINDOW)) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unavailable("Home maintenance window is unreadable")),
        };
        let fields: Vec<_> = encoded.lines().collect();
        let ["2", epoch, phase, boot, contest] = fields.as_slice() else {
            return Err(rejected("Home maintenance window is invalid"));
        };
        let epoch = epoch
            .parse()
            .map_err(|_| rejected("Home maintenance epoch is invalid"))?;
        super::require_epoch(epoch)?;
        if !session::valid_boot_id(boot)
            || !matches!(*phase, "draining" | "maintaining")
            || (contest != &"-"
                && (contest.is_empty() || !contest.bytes().all(|c| c.is_ascii_alphanumeric())))
        {
            return Err(rejected("Home maintenance window is invalid"));
        }
        Ok(Some(Self {
            epoch,
            boot: (*boot).to_owned(),
            contest_id: (*contest != "-").then(|| (*contest).to_owned()),
            maintaining: *phase == "maintaining",
        }))
    }

    fn persist(&self, root: &Path) -> Result<(), ResourceControlError> {
        super::write_state(
            root,
            WINDOW,
            &format!(
                "2\n{}\n{}\n{}\n{}\n",
                self.epoch,
                if self.maintaining {
                    "maintaining"
                } else {
                    "draining"
                },
                self.boot,
                self.contest_id.as_deref().unwrap_or("-")
            ),
        )
    }

    fn captured(&self) -> Option<GraphicalSession> {
        self.contest_id.as_ref().map(|id| GraphicalSession {
            boot_id: self.boot.clone(),
            logind_session_id: id.clone(),
        })
    }
}

pub(crate) fn require_closed(root: &Path) -> Result<(), ResourceControlError> {
    // Even damaged records close the gate; parsing is a recovery concern.
    match fs::symlink_metadata(super::state_directory(root).join(WINDOW)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(rejected("Home maintenance window is open")),
        Err(_) => Err(unavailable("Home maintenance window cannot be inspected")),
    }?;
    if let Some(progress) = super::query(root)?
        && (progress.phase != HomeResetPhase::Verified
            || !super::cleanup::complete(root, progress.reset_epoch)?)
    {
        return Err(rejected("native reset completion has not been established"));
    }
    Ok(())
}

pub(crate) fn query(root: &Path) -> Result<Option<HomeResetProgress>, ResourceControlError> {
    let mut progress = super::query(root)?;
    if let Some(window) = Window::read(root)?
        && progress
            .as_ref()
            .is_none_or(|p| p.reset_epoch != window.epoch)
    {
        progress = Some(HomeResetProgress {
            reset_epoch: window.epoch,
            phase: HomeResetPhase::Draining,
        });
    }
    if let Some(progress) = &mut progress
        && progress.phase == HomeResetPhase::Verified
    {
        super::cleanup::require_policy(root)?;
        if !super::cleanup::complete(root, progress.reset_epoch)? {
            progress.phase = HomeResetPhase::RecoveryRequired;
        }
    }
    Ok(progress)
}

async fn enter(
    root: &Path,
    connection: &Connection,
    window: &mut Window,
) -> Result<admission::MaintenanceGuard, ResourceControlError> {
    admission::close(root)?;
    session::drain_contest(connection, root, window.captured().as_ref()).await?;
    let guard = admission::maintenance(root)?;
    // The shared PAM lock now excludes a late worker registration. The drain
    // proved session, user manager and UID processes gone before any Home write.
    if !session::contest_absent(connection, root).await?
        || !session::contest_user_absent(connection, root).await?
        || !crate::processes::owned_by(
            root,
            session::account(root, session::account_name(SessionRole::Contest))?.uid,
        )?
        .is_empty()
    {
        return Err(unavailable(
            "contest drainage changed before Home maintenance",
        ));
    }
    if !window.maintaining {
        window.maintaining = true;
        window.persist(root)?;
    }
    super::cleanup::run(root, window.epoch)?;
    Ok(guard)
}

fn require_epoch(root: &Path, epoch: u64) -> Result<Option<Window>, ResourceControlError> {
    super::require_epoch(epoch)?;
    if let Err(error) = super::cleanup::require_policy(root) {
        admission::close(root)?;
        return Err(error);
    }
    let window = Window::read(root)?;
    if window.as_ref().is_some_and(|window| window.epoch != epoch)
        || super::query(root)?.is_some_and(|p| {
            p.reset_epoch > epoch || (p.reset_epoch != epoch && p.phase != HomeResetPhase::Verified)
        })
    {
        return Err(rejected("another Home maintenance epoch is in progress"));
    }
    Ok(window)
}

pub(crate) async fn prepare(
    root: &Path,
    connection: &Connection,
    epoch: u64,
    waiting: &GraphicalSession,
) -> Result<(), ResourceControlError> {
    let _mutation = admission::mutation(root)?;
    let owned = require_epoch(root, epoch)?;
    if owned.is_none() && verified_mount(root, epoch)? {
        return admission::open(root);
    }
    if owned.is_none() && super::cleanup::complete(root, epoch)? {
        // Replaying an already-cleaned epoch does not authorize capturing a new
        // contest just because its mount was lost. Recover must prove absence.
        admission::close(root)?;
        return Err(unavailable("completed reset requires Home mount recovery"));
    }
    if owned.is_none()
        && super::query(root)?
            .is_some_and(|p| p.reset_epoch == epoch && p.phase == HomeResetPhase::Verified)
        && !super::cleanup::complete(root, epoch)?
    {
        admission::close(root)?;
        return Err(rejected(
            "legacy Home completion requires an explicit new reset epoch",
        ));
    }
    if let Err(error) = super::prepare_metadata(root, epoch) {
        admission::close(root)?;
        return Err(error);
    }
    let mut window = if let Some(window) = owned {
        window
    } else {
        let observed = session::observe(connection, root).await?;
        if observed.waiting.state != GraphicalSessionState::Running
            || observed.waiting.session.as_ref() != Some(waiting)
            || !observed.waiting.desktop_ready
            || observed.waiting.locked_hint
            || observed.foreground != SessionForeground::Waiting
            || matches!(
                observed.contest.state,
                GraphicalSessionState::Ambiguous | GraphicalSessionState::Error
            )
        {
            return Err(rejected(
                "Home reset requires the captured ready waiting foreground and an unambiguous contest",
            ));
        }
        let window = Window {
            epoch,
            boot: session::read_boot_id(root)?,
            contest_id: observed
                .contest
                .session
                .map(|session| session.logind_session_id),
            maintaining: false,
        };
        window.persist(root)?;
        window
    };
    let _admission = enter(root, connection, &mut window).await?;
    super::prepare(root, epoch)
}

pub(crate) async fn apply(
    root: &Path,
    connection: &Connection,
    epoch: u64,
    worked: &mut bool,
) -> Result<(), ResourceControlError> {
    let _mutation = admission::mutation(root)?;
    let window = require_epoch(root, epoch)?;
    if window.is_none() && verified_mount(root, epoch)? {
        return Ok(());
    }
    *worked = true;
    let mut window =
        window.ok_or_else(|| rejected("Home maintenance window has not been prepared"))?;
    let _admission = enter(root, connection, &mut window).await?;
    super::apply(root, epoch)
}

fn verified_mount(root: &Path, epoch: u64) -> Result<bool, ResourceControlError> {
    if !super::query(root)?
        .is_some_and(|p| p.reset_epoch == epoch && p.phase == HomeResetPhase::Verified)
    {
        return Ok(false);
    }
    if !super::cleanup::complete(root, epoch)? {
        admission::close(root)?;
        return Ok(false);
    }
    match super::mounted_generation(root, epoch) {
        Ok(mounted) => Ok(mounted),
        Err(error) => {
            admission::close(root)?;
            Err(error)
        }
    }
}

pub(crate) async fn recover(
    root: &Path,
    connection: &Connection,
    epoch: u64,
    task: &mut Option<ResetTask>,
) -> Result<(), ResourceControlError> {
    super::require_epoch(epoch)?;
    if task_running(task, epoch).await? {
        return Ok(());
    }
    let mutation = admission::mutation(root)?;
    recover_owned(
        root,
        connection,
        epoch,
        mutation,
        task,
        super::remove_other_generations,
    )
    .await
}

async fn recover_owned(
    root: &Path,
    connection: &Connection,
    epoch: u64,
    mutation: admission::MutationGuard,
    task: &mut Option<ResetTask>,
    remove: impl FnOnce(&Path, u64) -> Result<(), ResourceControlError> + Send + 'static,
) -> Result<(), ResourceControlError> {
    let owned = require_epoch(root, epoch)?;
    if verified_mount(root, epoch)? {
        return finish(root, epoch);
    }
    let mut window = if let Some(window) = owned {
        window
    } else {
        if super::require_target_progress(root, epoch)? == HomeResetPhase::Verified
            && !super::cleanup::complete(root, epoch)?
        {
            admission::close(root)?;
            return Err(rejected(
                "legacy Home completion requires an explicit new reset epoch",
            ));
        }
        // Mount recovery after a new boot owns the durable generation, but has
        // no authority to terminate an unexpected new session.
        if !session::contest_absent(connection, root).await? {
            return Err(rejected("Home recovery found an uncaptured contest"));
        }
        let window = Window {
            epoch,
            boot: session::read_boot_id(root)?,
            contest_id: None,
            maintaining: false,
        };
        window.persist(root)?;
        window
    };
    let guard = enter(root, connection, &mut window).await?;
    let progress = super::query(root)?;
    if progress
        .as_ref()
        .is_none_or(|p| p.reset_epoch != epoch || p.phase == HomeResetPhase::Prepared)
    {
        super::prepare(root, epoch)?;
    }
    super::recover(root, epoch)?;
    if !super::mounted_generation(root, epoch)? {
        return Err(unavailable("Home mount recovery did not verify"));
    }
    *task = Some(start_task(root, epoch, mutation, guard, remove)?);
    Ok(())
}

pub(crate) async fn verify(
    root: &Path,
    connection: &Connection,
    epoch: u64,
    worked: &mut bool,
    task: &mut Option<ResetTask>,
) -> Result<HomeResetProgress, ResourceControlError> {
    verify_with_removal(
        root,
        connection,
        epoch,
        worked,
        task,
        super::remove_other_generations,
    )
    .await
}

async fn verify_with_removal(
    root: &Path,
    connection: &Connection,
    epoch: u64,
    worked: &mut bool,
    task: &mut Option<ResetTask>,
    remove: impl FnOnce(&Path, u64) -> Result<(), ResourceControlError> + Send + 'static,
) -> Result<HomeResetProgress, ResourceControlError> {
    // GDM preparation holds the mutation lock while it waits for the desktop.
    // Observing a completed, still mounted generation needs no mutation. Check
    // admission on both sides so an in-progress Home window cannot use this path;
    // incomplete publication must still take the lock and finish below.
    super::require_epoch(epoch)?;
    super::cleanup::require_policy(root)?;
    if task_running(task, epoch).await? {
        return Ok(HomeResetProgress {
            reset_epoch: epoch,
            phase: HomeResetPhase::InProgress,
        });
    }
    if admission::require_open(root).is_ok()
        && verified_mount(root, epoch)?
        && admission::require_open(root).is_ok()
    {
        return Ok(HomeResetProgress {
            reset_epoch: epoch,
            phase: HomeResetPhase::Verified,
        });
    }
    *worked = true;
    let mutation = admission::mutation(root)?;
    let owned = require_epoch(root, epoch)?;
    if query(root)?.is_some_and(|p| p.reset_epoch == epoch && p.phase == HomeResetPhase::Draining) {
        return Ok(HomeResetProgress {
            reset_epoch: epoch,
            phase: HomeResetPhase::Draining,
        });
    }
    let guard = if let Some(mut window) = owned {
        Some(enter(root, connection, &mut window).await?)
    } else {
        if !super::cleanup::complete(root, epoch)? {
            admission::close(root)?;
            return Err(rejected("native reset completion has not been established"));
        }
        None
    };
    let phase = super::require_target_progress(root, epoch)?;
    if phase != HomeResetPhase::Prepared
        && phase != HomeResetPhase::Verified
        && super::mounted_generation(root, epoch)?
    {
        let guard =
            guard.ok_or_else(|| rejected("Home maintenance window has not been prepared"))?;
        *task = Some(start_task(root, epoch, mutation, guard, remove)?);
        return Ok(HomeResetProgress {
            reset_epoch: epoch,
            phase: HomeResetPhase::InProgress,
        });
    }
    let progress = super::verify(root, epoch)?;
    drop(guard);
    if progress.phase == HomeResetPhase::Verified {
        finish(root, epoch)?;
    } else {
        admission::close(root)?;
    }
    Ok(progress)
}

fn finish(root: &Path, epoch: u64) -> Result<(), ResourceControlError> {
    if !super::cleanup::complete(root, epoch)? {
        admission::close(root)?;
        return Err(rejected("native reset completion has not been established"));
    }
    if let Some(window) = Window::read(root)? {
        if window.epoch != epoch
            || super::require_target_progress(root, epoch)? != HomeResetPhase::Verified
        {
            return Err(rejected("Home maintenance epoch has not been verified"));
        }
        fs::remove_file(super::state_directory(root).join(WINDOW))
            .map_err(|_| unavailable("Home maintenance window cannot be completed"))?;
        super::sync_directory(&super::state_directory(root))?;
    }
    // Durable completion precedes permission publication. A failure here retries
    // verification of the same generation; it never creates a fresh Home.
    admission::open(root)
}

pub(crate) async fn restore_home(
    root: &Path,
    connection: &Connection,
    observation: &mut RecoveryObservation,
    task: &mut Option<ResetTask>,
) -> Result<(), ResourceControlError> {
    restore_with_removal(
        root,
        connection,
        observation,
        task,
        super::remove_other_generations,
    )
    .await
}

async fn restore_with_removal(
    root: &Path,
    connection: &Connection,
    observation: &mut RecoveryObservation,
    task: &mut Option<ResetTask>,
    remove: impl FnOnce(&Path, u64) -> Result<(), ResourceControlError> + Send + 'static,
) -> Result<(), ResourceControlError> {
    if task.as_ref().and_then(ResetTask::progress).is_some()
        && let Some(progress) = query(root)?
    {
        observation.action = Some(RecoveryAction::Resume);
        observation.reset_epoch = Some(progress.reset_epoch);
        if task_running(task, progress.reset_epoch).await? {
            return Ok(());
        }
    }
    let mutation = admission::mutation(root)?;
    admission::close(root)?;
    super::cleanup::require_policy(root)?;
    if let Some(progress) = query(root)? {
        observation.action = Some(RecoveryAction::Resume);
        observation.reset_epoch = Some(progress.reset_epoch);
        task_running(task, progress.reset_epoch).await?;
        return recover_owned(
            root,
            connection,
            progress.reset_epoch,
            mutation,
            task,
            remove,
        )
        .await;
    }
    observation.action = Some(RecoveryAction::Baseline);
    // A fresh image has no remote reset epoch. Validate its baseline without
    // inventing a reset or silently adopting an unmanaged mount.
    super::prepare_metadata(root, 1)?;
    if super::state_exists(root)?
        || procfs::process::Process::myself()
            .and_then(|p| p.mountinfo())
            .map_err(|_| unavailable("Home mount state is unavailable"))?
            .iter()
            .any(|mount| mount.mount_point == root.join(super::CONTEST_HOME_RELATIVE_PATH))
    {
        return Err(rejected(
            "initial Home contains unowned state or an unmanaged mount",
        ));
    }
    admission::open(root)
}

#[cfg(test)]
mod tests {
    use super::super::cleanup::tests::TestError;
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    const BOOT: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn fixture() -> Result<tempfile::TempDir, TestError> {
        let root = tempfile::tempdir()?;
        super::super::cleanup::tests::policy_fixture(root.path())?;
        fs::create_dir_all(super::super::state_directory(root.path()))?;
        let runtime = root.path().join("run/natsume-privileged");
        fs::create_dir_all(&runtime)?;
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;
        fs::create_dir_all(root.path().join("proc/sys/kernel/random"))?;
        fs::write(root.path().join("proc/sys/kernel/random/boot_id"), BOOT)?;
        Ok(root)
    }

    #[test]
    fn durable_window_withdraws_even_a_previously_published_permit() -> Result<(), TestError> {
        let root = fixture()?;
        admission::open(root.path())?;
        admission::require_open(root.path())?;
        let window = Window {
            epoch: 7,
            boot: BOOT.to_owned(),
            contest_id: Some("c9".to_owned()),
            maintaining: false,
        };
        window.persist(root.path())?;
        assert_eq!(Window::read(root.path())?, Some(window));
        assert!(admission::require_open(root.path()).is_err());
        assert!(admission::open(root.path()).is_err());
        assert_eq!(
            query(root.path())?,
            Some(HomeResetProgress {
                reset_epoch: 7,
                phase: HomeResetPhase::Draining
            })
        );
        assert!(require_epoch(root.path(), 8).is_err());
        assert!(finish(root.path(), 7).is_err());
        assert!(Window::read(root.path())?.is_some());
        Ok(())
    }

    #[test]
    fn invalid_windows_stay_closed_and_are_not_rewritten() -> Result<(), TestError> {
        let root = fixture()?;
        for invalid in [
            "9\n7\ndraining\n66f2fc8e-5672-4fb4-8a84-ced3d96c83f6\nc9\n",
            "2\n7\ndraining\nbad-boot\nc9\n",
            "garbage",
        ] {
            super::super::write_state(root.path(), WINDOW, invalid)?;
            assert!(Window::read(root.path()).is_err());
            assert!(require_closed(root.path()).is_err());
            assert!(admission::open(root.path()).is_err());
            assert_eq!(
                fs::read_to_string(super::super::state_directory(root.path()).join(WINDOW))?,
                invalid
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_verified_marker_without_a_mount_is_not_ready_while_mutation_is_busy()
    -> Result<(), TestError> {
        let root = fixture()?;
        fs::create_dir_all(root.path().join("proc/self"))?;
        fs::write(root.path().join("proc/self/mountinfo"), "")?;
        admission::open(root.path())?;
        super::super::write_progress(root.path(), 7, HomeResetPhase::Verified)?;
        let _mutation = admission::mutation(root.path())?;

        let (left, right) = tokio::net::UnixStream::pair()?;
        let guid = zbus::Guid::generate();
        let (connection, _peer) = tokio::try_join!(
            zbus::connection::Builder::unix_stream(left.into_std()?)
                .server(guid)
                .p2p()
                .build(),
            zbus::connection::Builder::unix_stream(right.into_std()?)
                .p2p()
                .build(),
        )?;
        assert!(matches!(
            verify(root.path(), &connection, 7, &mut false, &mut None).await,
            Err(ResourceControlError::Unavailable(_))
        ));
        assert_eq!(
            super::super::query(root.path())?.map(|progress| progress.phase),
            Some(HomeResetPhase::Verified)
        );
        assert!(admission::require_open(root.path()).is_err());
        Ok(())
    }

    async fn peer() -> Result<(Connection, Connection), TestError> {
        let (left, right) = tokio::net::UnixStream::pair()?;
        Ok(tokio::try_join!(
            zbus::connection::Builder::unix_stream(left.into_std()?)
                .server(zbus::Guid::generate())
                .p2p()
                .build(),
            zbus::connection::Builder::unix_stream(right.into_std()?)
                .p2p()
                .build(),
        )?)
    }

    fn verified_fixture() -> Result<tempfile::TempDir, TestError> {
        let root = super::super::tests::fixture();
        super::super::cleanup::tests::policy_fixture(root.path())?;
        let runtime = root.path().join("run/natsume-privileged");
        fs::create_dir_all(&runtime)?;
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;
        fs::create_dir_all(root.path().join("proc/sys/kernel/random"))?;
        fs::write(root.path().join("proc/sys/kernel/random/boot_id"), BOOT)?;
        super::super::prepare(root.path(), 7)?;
        super::super::apply(root.path(), 7)?;
        super::super::record_verification(root.path(), 7, true)?;
        let mounts = root.path().join("proc/self/mountinfo");
        let original = fs::read_to_string(&mounts)?;
        fs::write(
            mounts,
            format!(
                "{original}43 25 0:44 / {} rw - overlay overlay rw,lowerdir={},upperdir={},workdir={}\n",
                root.path().join("home/teams").display(),
                root.path()
                    .join(super::super::TEMPLATE_RELATIVE_PATH)
                    .display(),
                super::super::generation_directory(root.path(), 7)
                    .join("upper")
                    .display(),
                super::super::generation_directory(root.path(), 7)
                    .join("work")
                    .display()
            ),
        )?;
        Ok(root)
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep the legacy completion checks across all entry points together"
    )]
    async fn legacy_verified_cannot_bypass_any_entry_point_or_existing_permit()
    -> Result<(), TestError> {
        let root = verified_fixture()?;
        let (connection, _peer) = peer().await?;
        let waiting = GraphicalSession {
            boot_id: BOOT.to_owned(),
            logind_session_id: "newwaiting".to_owned(),
        };
        fs::write(
            root.path().join("run/natsume-privileged/contest-permit"),
            BOOT,
        )?;
        fs::write(root.path().join("home/teams/current"), "keep")?;
        assert!(admission::require_open(root.path()).is_err());
        assert!(admission::open(root.path()).is_err());
        assert_eq!(
            query(root.path())?.map(|p| p.phase),
            Some(HomeResetPhase::RecoveryRequired)
        );
        assert!(
            prepare(root.path(), &connection, 7, &waiting)
                .await
                .is_err()
        );
        assert!(
            apply(root.path(), &connection, 7, &mut false)
                .await
                .is_err()
        );
        assert!(
            verify(root.path(), &connection, 7, &mut false, &mut None)
                .await
                .is_err()
        );
        assert!(
            recover(root.path(), &connection, 7, &mut None)
                .await
                .is_err()
        );
        assert!(
            restore_home(
                root.path(),
                &connection,
                &mut RecoveryObservation::default(),
                &mut None
            )
            .await
            .is_err()
        );
        assert!(Window::read(root.path())?.is_none());
        assert!(require_epoch(root.path(), 8)?.is_none());
        assert_eq!(
            fs::read_to_string(root.path().join("home/teams/current"))?,
            "keep"
        );
        assert_eq!(
            super::super::query(root.path())?.map(|p| p.phase),
            Some(HomeResetPhase::Verified)
        );
        // The former CLEAR-only proof is parsed as legacy, not malformed, so
        // a new explicit epoch remains possible but no old fast path can admit.
        super::super::write_state(root.path(), "cleanup-complete", "1\n7\n")?;
        assert!(admission::require_open(root.path()).is_err());
        assert!(
            prepare(root.path(), &connection, 7, &waiting)
                .await
                .is_err()
        );
        assert!(
            apply(root.path(), &connection, 7, &mut false)
                .await
                .is_err()
        );
        assert!(
            verify(root.path(), &connection, 7, &mut false, &mut None)
                .await
                .is_err()
        );
        assert!(
            recover(root.path(), &connection, 7, &mut None)
                .await
                .is_err()
        );
        assert!(
            restore_home(
                root.path(),
                &connection,
                &mut RecoveryObservation::default(),
                &mut None
            )
            .await
            .is_err()
        );
        assert!(require_epoch(root.path(), 8)?.is_none());
        // Nor may a crash-persisted Home Verified inside an owned window grant admission.
        Window {
            epoch: 7,
            boot: BOOT.to_owned(),
            contest_id: Some("old".to_owned()),
            maintaining: true,
        }
        .persist(root.path())?;
        assert_eq!(
            query(root.path())?.map(|p| p.phase),
            Some(HomeResetPhase::RecoveryRequired)
        );
        assert!(finish(root.path(), 7).is_err());
        assert!(Window::read(root.path())?.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn completed_prepare_with_lost_mount_cannot_capture_a_new_contest()
    -> Result<(), TestError> {
        let root = verified_fixture()?;
        let (connection, _peer) = peer().await?;
        super::super::write_state(root.path(), "cleanup-complete", "2\n7\n")?;
        let mounts = root.path().join("proc/self/mountinfo");
        let lower_only: String = fs::read_to_string(&mounts)?
            .lines()
            .filter(|line| !line.contains(" - overlay "))
            .fold(String::new(), |mut encoded, line| {
                encoded.push_str(line);
                encoded.push('\n');
                encoded
            });
        fs::write(mounts, lower_only)?;
        fs::write(root.path().join("home/teams/current"), "new Home state")?;
        let waiting = GraphicalSession {
            boot_id: BOOT.to_owned(),
            logind_session_id: "waiting".to_owned(),
        };
        for phase in [HomeResetPhase::Verified, HomeResetPhase::RecoveryRequired] {
            super::super::write_progress(root.path(), 7, phase)?;
            assert!(matches!(
                prepare(root.path(), &connection, 7, &waiting).await,
                Err(ResourceControlError::Unavailable(message)) if message == "completed reset requires Home mount recovery"
            ));
            assert!(Window::read(root.path())?.is_none());
            assert!(admission::require_open(root.path()).is_err());
        }
        assert_eq!(
            fs::read_to_string(root.path().join("home/teams/current"))?,
            "new Home state"
        );
        Ok(())
    }

    #[tokio::test]
    async fn full_scope_replays_and_healthy_restart_never_drain_or_reclean_current_writes()
    -> Result<(), TestError> {
        let root = verified_fixture()?;
        let (connection, _peer) = peer().await?; // Deliberately no logind/cleanup services.
        super::super::write_state(root.path(), "cleanup-complete", "2\n7\n")?;
        fs::create_dir(root.path().join("tmp"))?;
        fs::write(root.path().join("tmp/current"), "new temporary state")?;
        fs::write(root.path().join("home/teams/current"), "new Home state")?;
        let waiting = GraphicalSession {
            boot_id: BOOT.to_owned(),
            logind_session_id: "waiting".to_owned(),
        };
        for _ in 0..2 {
            prepare(root.path(), &connection, 7, &waiting).await?;
            apply(root.path(), &connection, 7, &mut false).await?;
            assert_eq!(
                verify(root.path(), &connection, 7, &mut false, &mut None)
                    .await?
                    .phase,
                HomeResetPhase::Verified
            );
            recover(root.path(), &connection, 7, &mut None).await?;
            restore_home(
                root.path(),
                &connection,
                &mut RecoveryObservation::default(),
                &mut None,
            )
            .await?;
            assert_eq!(
                fs::read_to_string(root.path().join("tmp/current"))?,
                "new temporary state"
            );
            assert_eq!(
                fs::read_to_string(root.path().join("home/teams/current"))?,
                "new Home state"
            );
            admission::require_open(root.path())?;
        }
        // Crash after proof publication, before window deletion / permit publication.
        admission::close(root.path())?;
        Window {
            epoch: 7,
            boot: BOOT.to_owned(),
            contest_id: Some("old".to_owned()),
            maintaining: true,
        }
        .persist(root.path())?;
        recover(root.path(), &connection, 7, &mut None).await?;
        assert!(Window::read(root.path())?.is_none());
        admission::require_open(root.path())?;
        assert_eq!(
            fs::read_to_string(root.path().join("tmp/current"))?,
            "new temporary state"
        );
        // Removing or changing the image contract revokes even a current permit.
        fs::write(root.path().join("etc/natsume/contest-reset-policy"), "2\n")?;
        assert!(admission::require_open(root.path()).is_err());
        assert!(
            verify(root.path(), &connection, 7, &mut false, &mut None)
                .await
                .is_err()
        );
        Ok(())
    }
    #[tokio::test]
    async fn request_events_and_healthy_replays_keep_the_original_admission_order() {
        use crate::tests::{Capture, Value, assert_text, subscriber_guard};
        use tracing::instrument::WithSubscriber as _;
        let _guard = subscriber_guard();
        let root = verified_fixture().unwrap_or_else(|e| panic!("fixture: {e}"));
        super::super::write_state(root.path(), "cleanup-complete", "2\n7\n")
            .unwrap_or_else(|e| panic!("proof: {e}"));
        admission::open(root.path()).unwrap_or_else(|e| panic!("permit: {e}"));
        let (connection, _peer) = peer().await.unwrap_or_else(|e| panic!("peer: {e}"));
        let mut service = crate::PrivilegedService {
            filesystem_root: root.path().to_owned(),
            home_reset_task: None,
        };
        let capture = Capture::default();
        async {
            service
                .verify_home_reset(7, &connection)
                .await
                .unwrap_or_else(|e| panic!("verify: {e}"));
            service
                .apply_home_reset(7, &connection)
                .await
                .unwrap_or_else(|e| panic!("apply: {e}"));
            assert!(capture.events("home reset completed").is_empty());
            let waiting = GraphicalSession {
                boot_id: BOOT.to_owned(),
                logind_session_id: "waiting-canary".to_owned(),
            };
            assert!(
                service
                    .prepare_home_reset(0, waiting.clone(), &connection)
                    .await
                    .is_err()
            );
            assert!(service.apply_home_reset(0, &connection).await.is_err());
            assert!(service.verify_home_reset(0, &connection).await.is_err());
            assert!(service.recover_home_reset(0, &connection).await.is_err());
            let events = capture.events("home reset completed");
            assert_eq!(events.len(), 4);
            for (event, operation) in events.iter().zip([
                "prepare_request",
                "apply_request",
                "verify_request",
                "recover_request",
            ]) {
                assert_text(event, "operation", operation);
                assert_text(event, "result", "error");
                assert_eq!(event.fields.get("reset_epoch"), Some(&Value::Unsigned(0)));
                assert!(matches!(
                    event.fields.get("elapsed_us"),
                    Some(Value::Unsigned(_))
                ));
            }
            let stale = GraphicalSession {
                boot_id: "550e8400-e29b-41d4-a716-446655440001".to_owned(),
                logind_session_id: "activation-canary".to_owned(),
            };
            assert!(
                service
                    .activate_session(SessionRole::Waiting, stale, &connection)
                    .await
                    .is_err()
            );
            let events = capture.events("session switch completed");
            assert_eq!(events.len(), 1);
            assert_text(&events[0], "operation", "activate");
            assert_text(&events[0], "foreground_target", "waiting");
            assert_text(&events[0], "result", "error");
            assert!(matches!(
                events[0].fields.get("activate_us"),
                Some(Value::Unsigned(_))
            ));
            admission::require_open(root.path()).unwrap_or_else(|e| panic!("original permit: {e}"));
            for event in events {
                assert!(!format!("{event:?}").contains("activation-canary"));
            }
        }
        .with_subscriber(capture.subscriber())
        .await;
    }

    #[tokio::test]
    async fn recovery_event_captures_resume_baseline_and_unknown_action_on_corruption() {
        use crate::tests::{Capture, Value, assert_text, subscriber_guard};
        use tracing::instrument::WithSubscriber as _;
        let _guard = subscriber_guard();
        let (connection, _peer) = peer().await.unwrap_or_else(|e| panic!("peer: {e}"));
        for action in ["resume", "baseline", "corrupt"] {
            let root = if action == "resume" {
                verified_fixture().unwrap_or_else(|e| panic!("verified: {e}"))
            } else {
                super::super::tests::fixture()
            };
            super::super::cleanup::tests::policy_fixture(root.path())
                .unwrap_or_else(|e| panic!("policy: {e}"));
            fs::create_dir_all(root.path().join("run/natsume-privileged"))
                .unwrap_or_else(|e| panic!("runtime: {e}"));
            fs::set_permissions(
                root.path().join("run/natsume-privileged"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap_or_else(|e| panic!("mode: {e}"));
            fs::create_dir_all(root.path().join("proc/sys/kernel/random"))
                .unwrap_or_else(|e| panic!("proc: {e}"));
            fs::write(root.path().join("proc/sys/kernel/random/boot_id"), BOOT)
                .unwrap_or_else(|e| panic!("boot: {e}"));
            if action == "resume" {
                super::super::write_state(root.path(), "cleanup-complete", "2\n7\n")
                    .unwrap_or_else(|e| panic!("proof: {e}"));
            }
            if action == "corrupt" {
                fs::write(
                    super::super::state_directory(root.path()).join("progress"),
                    "corrupt-progress-canary",
                )
                .unwrap_or_else(|e| panic!("progress: {e}"));
            }
            let capture = Capture::default();
            let mut service = crate::PrivilegedService {
                filesystem_root: root.path().to_owned(),
                home_reset_task: None,
            };
            async {
                let result = service.recover_local_home(&connection).await;
                assert_eq!(result.is_ok(), action != "corrupt");
                let events = capture.events("home recovery");
                assert_eq!(events.len(), 1);
                assert_text(
                    &events[0],
                    "result",
                    if action == "corrupt" { "error" } else { "ok" },
                );
                if action == "corrupt" {
                    assert!(!events[0].fields.contains_key("action"));
                    assert!(!events[0].fields.contains_key("reset_epoch"));
                } else {
                    assert_text(&events[0], "action", action);
                    assert_eq!(
                        events[0].fields.get("reset_epoch"),
                        if action == "resume" {
                            Some(&Value::Unsigned(7))
                        } else {
                            None
                        }
                    );
                    admission::require_open(root.path()).unwrap_or_else(|e| panic!("ready: {e}"));
                }
                assert!(!format!("{:?}", events[0]).contains("corrupt-progress-canary"));
            }
            .with_subscriber(capture.subscriber())
            .await;
        }
    }
    struct AbsentLogind {
        sessions: ListedSessions,
        uid: u32,
    }
    struct InactiveSystemd {
        unit: zbus::OwnedObjectPath,
        job: zbus::OwnedObjectPath,
    }
    struct InactiveUnit {
        state: &'static str,
    }

    #[derive(Debug, zbus::DBusError)]
    #[zbus(prefix = "org.freedesktop.login1")]
    enum UserError {
        NoSuchUser(String),
    }

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl AbsentLogind {
        fn list_sessions(&self) -> ListedSessions {
            self.sessions.clone()
        }

        fn get_user(&self, uid: u32) -> Result<zbus::OwnedObjectPath, UserError> {
            assert_eq!(uid, self.uid);
            Err(UserError::NoSuchUser("absent".to_owned()))
        }
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl InactiveSystemd {
        fn load_unit(&self, name: &str) -> zbus::OwnedObjectPath {
            assert_ne!(name, "");
            self.unit.clone()
        }

        fn stop_unit(&self, name: &str, mode: &str) -> zbus::OwnedObjectPath {
            assert_ne!(name, "");
            assert_eq!(mode, "replace");
            self.job.clone()
        }
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Unit")]
    impl InactiveUnit {
        #[zbus(property)]
        fn active_state(&self) -> &str {
            self.state
        }
    }

    async fn maintenance_peer() -> Result<(Connection, Connection), TestError> {
        let (left, right) = tokio::net::UnixStream::pair()?;
        let helper = zbus::connection::Builder::unix_stream(left.into_std()?)
            .server(zbus::Guid::generate())
            .p2p()
            .serve_at("/reset/ready", InactiveUnit { state: "inactive" });
        let uid = if rustix::process::geteuid().is_root() {
            1001
        } else {
            rustix::process::geteuid().as_raw()
        };
        let system = zbus::connection::Builder::unix_stream(right.into_std()?)
            .p2p()
            .serve_at(
                "/org/freedesktop/login1",
                AbsentLogind {
                    sessions: Vec::new(),
                    uid,
                },
            )
            .serve_at(
                "/org/freedesktop/systemd1",
                InactiveSystemd {
                    unit: zbus::OwnedObjectPath::try_from("/reset/unit")
                        .unwrap_or_else(|error| panic!("unit path: {error}")),
                    job: zbus::OwnedObjectPath::try_from("/reset/job")
                        .unwrap_or_else(|error| panic!("job path: {error}")),
                },
            )
            .serve_at("/reset/unit", InactiveUnit { state: "inactive" });
        Ok(tokio::try_join!(helper.build(), system.build())?)
    }

    fn pending_fixture(phase: HomeResetPhase) -> Result<tempfile::TempDir, TestError> {
        let root = verified_fixture()?;
        super::super::write_state(root.path(), "cleanup-complete", "2\n7\n")?;
        super::super::write_progress(root.path(), 7, phase)?;
        Window {
            epoch: 7,
            boot: BOOT.to_owned(),
            contest_id: Some("captured".to_owned()),
            maintaining: true,
        }
        .persist(root.path())?;
        let workers = root.path().join("run/natsume-privileged/contest-workers");
        fs::create_dir_all(&workers)?;
        fs::set_permissions(workers, fs::Permissions::from_mode(0o700))?;
        let old = super::super::generation_directory(root.path(), 6).join("upper");
        fs::create_dir_all(&old)?;
        fs::write(old.join("old"), b"previous generation")?;
        fs::write(
            root.path().join("home/teams/current"),
            b"current generation",
        )?;
        admission::close(root.path())?;
        Ok(root)
    }

    async fn wait_for_task(task: Option<&ResetTask>) {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while task.is_some_and(|task| !task.handle.is_finished()) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|error| panic!("background task did not finish: {error}"));
    }

    #[tokio::test]
    async fn concurrent_verification_keeps_one_task_and_queries_responsive() -> Result<(), TestError>
    {
        use natsume_workstation_ipc::{PRIVILEGED1_PATH, Privileged1Proxy};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        };

        let root = pending_fixture(HomeResetPhase::Applied)?;
        let (helper, client) = maintenance_peer().await?;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let (started, entered) = tokio::sync::oneshot::channel();
        let (release, blocked) = mpsc::channel();
        let mut task = None;
        let progress = verify_with_removal(
            root.path(),
            &helper,
            7,
            &mut false,
            &mut task,
            move |root, epoch| {
                counter.fetch_add(1, Ordering::SeqCst);
                let _sent = started.send(());
                let _released = blocked.recv();
                super::super::remove_other_generations(root, epoch)
            },
        )
        .await?;
        assert_eq!(progress.phase, HomeResetPhase::InProgress);
        entered
            .await
            .unwrap_or_else(|error| panic!("deletion did not start: {error}"));
        let service = crate::PrivilegedService {
            filesystem_root: root.path().to_owned(),
            home_reset_task: task,
        };
        helper.object_server().at(PRIVILEGED1_PATH, service).await?;
        let proxy = Privileged1Proxy::new(&client).await?;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let (left, right, query, ready) = tokio::join!(
                proxy.verify_home_reset(7),
                proxy.verify_home_reset(7),
                proxy.query_home_reset(),
                proxy.is_home_ready()
            );
            assert_eq!(left?.phase, HomeResetPhase::InProgress);
            assert_eq!(right?.phase, HomeResetPhase::InProgress);
            assert_eq!(query?.map(|p| p.phase), Some(HomeResetPhase::InProgress));
            assert!(!ready?);
            proxy.recover_home_reset(7).await?;
            proxy.recover_local_home().await?;
            assert!(proxy.verify_home_reset(8).await.is_err());
            Ok::<_, TestError>(())
        })
        .await
        .unwrap_or_else(|error| panic!("queries blocked behind deletion: {error}"))?;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(admission::mutation(root.path()).is_err());
        assert!(admission::maintenance(root.path()).is_err());
        assert!(admission::require_open(root.path()).is_err());
        release
            .send(())
            .unwrap_or_else(|error| panic!("release deletion: {error}"));
        let interface = helper
            .object_server()
            .interface::<_, crate::PrivilegedService>(PRIVILEGED1_PATH)
            .await?;
        {
            let service = interface.get().await;
            wait_for_task(service.home_reset_task.as_ref()).await;
        }
        assert_eq!(
            proxy.verify_home_reset(7).await?.phase,
            HomeResetPhase::Verified
        );
        assert!(!super::super::generation_directory(root.path(), 6).exists());
        assert!(Window::read(root.path())?.is_none());
        admission::require_open(root.path())?;
        Ok(())
    }

    #[tokio::test]
    async fn runtime_restore_closes_admission_until_background_completion() -> Result<(), TestError>
    {
        let root = pending_fixture(HomeResetPhase::Applied)?;
        let permit = root.path().join("run/natsume-privileged/contest-permit");
        fs::write(&permit, BOOT)?;
        let (connection, _peer) = maintenance_peer().await?;
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let mut observation = RecoveryObservation::default();
        let mut task = None;
        restore_with_removal(
            root.path(),
            &connection,
            &mut observation,
            &mut task,
            move |root, epoch| {
                let _sent = entered.send(());
                let _released = blocked.recv();
                super::super::remove_other_generations(root, epoch)
            },
        )
        .await?;
        started
            .await
            .unwrap_or_else(|error| panic!("deletion did not start: {error}"));
        assert_eq!(observation.reset_epoch, Some(7));
        assert!(!permit.exists());
        assert!(admission::require_open(root.path()).is_err());
        assert_eq!(
            query(root.path())?.map(|p| p.phase),
            Some(HomeResetPhase::InProgress)
        );
        restore_home(root.path(), &connection, &mut observation, &mut task).await?;
        let window = fs::read(super::super::state_directory(root.path()).join(WINDOW))?;
        Window {
            epoch: 8,
            boot: BOOT.to_owned(),
            contest_id: None,
            maintaining: true,
        }
        .persist(root.path())?;
        super::super::write_progress(root.path(), 8, HomeResetPhase::InProgress)?;
        assert!(matches!(
            restore_home(root.path(), &connection, &mut observation, &mut task).await,
            Err(ResourceControlError::Rejected(_))
        ));
        fs::write(
            super::super::state_directory(root.path()).join(WINDOW),
            window,
        )?;
        super::super::write_progress(root.path(), 7, HomeResetPhase::InProgress)?;
        release
            .send(())
            .unwrap_or_else(|error| panic!("release deletion: {error}"));
        wait_for_task(task.as_ref()).await;
        assert_eq!(
            verify(root.path(), &connection, 7, &mut false, &mut task)
                .await?
                .phase,
            HomeResetPhase::Verified
        );
        admission::require_open(root.path())?;
        Ok(())
    }

    #[tokio::test]
    async fn runtime_restore_with_invalid_policy_withdraws_existing_permit() -> Result<(), TestError>
    {
        let root = verified_fixture()?;
        super::super::write_state(root.path(), "cleanup-complete", "2\n7\n")?;
        admission::open(root.path())?;
        fs::write(
            root.path().join("etc/natsume/contest-reset-policy"),
            "invalid\n",
        )?;
        let (connection, _peer) = peer().await?;
        assert!(
            restore_home(
                root.path(),
                &connection,
                &mut RecoveryObservation::default(),
                &mut None
            )
            .await
            .is_err()
        );
        assert!(
            !root
                .path()
                .join("run/natsume-privileged/contest-permit")
                .exists()
        );
        Ok(())
    }

    #[tokio::test]
    async fn failed_removal_stays_pending_and_recovery_retries_the_same_epoch()
    -> Result<(), TestError> {
        let root = pending_fixture(HomeResetPhase::Applied)?;
        let (connection, _peer) = maintenance_peer().await?;
        let mut task = None;
        assert_eq!(
            verify_with_removal(
                root.path(),
                &connection,
                7,
                &mut false,
                &mut task,
                |root, _epoch| {
                    fs::remove_file(super::super::generation_directory(root, 6).join("upper/old"))
                        .map_err(|_| unavailable("fixture partial deletion failed"))?;
                    Err(unavailable("fixture removal failure"))
                }
            )
            .await?
            .phase,
            HomeResetPhase::InProgress
        );
        wait_for_task(task.as_ref()).await;
        assert_eq!(
            query(root.path())?.map(|p| (p.reset_epoch, p.phase)),
            Some((7, HomeResetPhase::InProgress))
        );
        assert!(Window::read(root.path())?.is_some());
        assert!(admission::require_open(root.path()).is_err());
        assert!(
            !root
                .path()
                .join("run/natsume-privileged/contest-permit")
                .exists()
        );
        assert!(matches!(
            verify(root.path(), &connection, 7, &mut false, &mut task).await,
            Err(ResourceControlError::Unavailable(_))
        ));
        assert!(task.is_none());
        recover(root.path(), &connection, 7, &mut task).await?;
        wait_for_task(task.as_ref()).await;
        assert_eq!(
            verify(root.path(), &connection, 7, &mut false, &mut task)
                .await?
                .phase,
            HomeResetPhase::Verified
        );
        assert!(!super::super::generation_directory(root.path(), 6).exists());
        assert_eq!(
            fs::read(root.path().join("home/teams/current"))?,
            b"current generation"
        );
        admission::require_open(root.path())?;
        Ok(())
    }

    #[tokio::test]
    async fn lost_mount_after_removal_prevents_completion_publication() -> Result<(), TestError> {
        let root = pending_fixture(HomeResetPhase::Applied)?;
        let (connection, _peer) = maintenance_peer().await?;
        let mut task = None;
        verify_with_removal(
            root.path(),
            &connection,
            7,
            &mut false,
            &mut task,
            |root, epoch| {
                super::super::remove_other_generations(root, epoch)?;
                fs::write(root.join("proc/self/mountinfo"), "")
                    .map_err(|_| unavailable("fixture mount loss failed"))
            },
        )
        .await?;
        wait_for_task(task.as_ref()).await;
        assert_eq!(
            query(root.path())?.map(|p| p.phase),
            Some(HomeResetPhase::InProgress)
        );
        assert!(Window::read(root.path())?.is_some());
        assert!(
            !root
                .path()
                .join("run/natsume-privileged/contest-permit")
                .exists()
        );
        assert!(matches!(
            verify(root.path(), &connection, 7, &mut false, &mut task).await,
            Err(ResourceControlError::Unavailable(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn startup_resumes_persisted_boundaries_without_waiting_for_verify()
    -> Result<(), TestError> {
        let (connection, _peer) = maintenance_peer().await?;
        for phase in [HomeResetPhase::Applied, HomeResetPhase::InProgress] {
            for removal_complete in [false, true] {
                let root = pending_fixture(phase)?;
                if removal_complete {
                    super::super::remove_other_generations(root.path(), 7)?;
                } else {
                    fs::remove_file(
                        super::super::generation_directory(root.path(), 6).join("upper/old"),
                    )?;
                }
                let captured = fs::read(super::super::state_directory(root.path()).join(WINDOW))?;
                let mut service = crate::PrivilegedService {
                    filesystem_root: root.path().to_owned(),
                    home_reset_task: None,
                };
                service.recover_local_home(&connection).await?;
                assert!(service.home_reset_task.is_some());
                wait_for_task(service.home_reset_task.as_ref()).await;
                assert_eq!(
                    query(root.path())?.map(|p| (p.reset_epoch, p.phase)),
                    Some((7, HomeResetPhase::Verified))
                );
                assert!(String::from_utf8_lossy(&captured).contains("captured"));
                assert!(Window::read(root.path())?.is_none());
                assert!(!super::super::generation_directory(root.path(), 6).exists());
                admission::require_open(root.path())?;
                assert_eq!(
                    service.verify_home_reset(7, &connection).await?.phase,
                    HomeResetPhase::Verified
                );
            }
        }
        Ok(())
    }

    type ListedSessions = Vec<(String, u32, String, String, zbus::OwnedObjectPath)>;
    struct SilentLogind;
    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl SilentLogind {
        async fn list_sessions(&self) -> ListedSessions {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn recovery_timeout_keeps_the_captured_action_and_epoch_after_future_cancellation() {
        use crate::tests::{Capture, Value, assert_text, subscriber_guard};
        use tracing::instrument::WithSubscriber as _;
        let _guard = subscriber_guard();
        let root = super::super::tests::fixture();
        super::super::cleanup::tests::policy_fixture(root.path())
            .unwrap_or_else(|e| panic!("policy: {e}"));
        let runtime = root.path().join("run/natsume-privileged");
        fs::create_dir_all(&runtime).unwrap_or_else(|e| panic!("runtime: {e}"));
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|e| panic!("mode: {e}"));
        fs::create_dir_all(root.path().join("proc/sys/kernel/random"))
            .unwrap_or_else(|e| panic!("proc: {e}"));
        fs::write(root.path().join("proc/sys/kernel/random/boot_id"), BOOT)
            .unwrap_or_else(|e| panic!("boot: {e}"));
        Window {
            epoch: 7,
            boot: BOOT.to_owned(),
            contest_id: Some("capturedcanary".to_owned()),
            maintaining: false,
        }
        .persist(root.path())
        .unwrap_or_else(|e| panic!("window: {e}"));
        let (server, client) =
            tokio::net::UnixStream::pair().unwrap_or_else(|e| panic!("pair: {e}"));
        let server = zbus::connection::Builder::unix_stream(
            server.into_std().unwrap_or_else(|e| panic!("server: {e}")),
        )
        .server(zbus::Guid::generate())
        .p2p()
        .serve_at("/org/freedesktop/login1", SilentLogind);
        let client = zbus::connection::Builder::unix_stream(
            client.into_std().unwrap_or_else(|e| panic!("client: {e}")),
        )
        .p2p();
        let (_server, connection) =
            tokio::try_join!(server.build(), client.build()).unwrap_or_else(|e| panic!("bus: {e}"));
        let mut service = crate::PrivilegedService {
            filesystem_root: root.path().to_owned(),
            home_reset_task: None,
        };
        let capture = Capture::default();
        async {
            assert!(matches!(service.recover_local_home(&connection).await, Err(ResourceControlError::Unavailable(_))));
            let events = capture.events("home recovery");
            assert_eq!(events.len(), 1);
            assert_text(&events[0], "action", "resume");
            assert_text(&events[0], "result", "timeout");
            assert_eq!(events[0].fields.get("reset_epoch"), Some(&Value::Unsigned(7)));
            assert!(matches!(events[0].fields.get("elapsed_us"), Some(Value::Unsigned(elapsed)) if *elapsed >= 8_000_000));
            assert!(!format!("{:?}", events[0]).contains("capturedcanary"));
            assert!(admission::require_open(root.path()).is_err());
            let _retry = admission::mutation(root.path()).unwrap_or_else(|e| panic!("released mutation: {e}"));
        }.with_subscriber(capture.subscriber()).await;
    }
}
