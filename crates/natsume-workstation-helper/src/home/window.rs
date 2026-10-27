use std::{fs, path::Path};

use natsume_workstation_ipc::{
    GraphicalSession, GraphicalSessionState, HomeResetPhase, HomeResetProgress,
    ResourceControlError, SessionForeground, SessionRole,
};
use zbus::Connection;

use super::{rejected, unavailable};
use crate::{admission, session};

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
) -> Result<(), ResourceControlError> {
    let _mutation = admission::mutation(root)?;
    let window = require_epoch(root, epoch)?;
    if window.is_none() && verified_mount(root, epoch)? {
        return Ok(());
    }
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
) -> Result<(), ResourceControlError> {
    let _mutation = admission::mutation(root)?;
    recover_owned(root, connection, epoch).await
}

async fn recover_owned(
    root: &Path,
    connection: &Connection,
    epoch: u64,
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
    let verified = super::verify(root, epoch)?;
    if verified.phase != HomeResetPhase::Verified {
        return Err(unavailable("Home mount recovery did not verify"));
    }
    drop(guard);
    finish(root, epoch)
}

pub(crate) async fn verify(
    root: &Path,
    connection: &Connection,
    epoch: u64,
) -> Result<HomeResetProgress, ResourceControlError> {
    // GDM preparation holds the mutation lock while it waits for the desktop.
    // Observing a completed, still mounted generation needs no mutation. Check
    // admission on both sides so an in-progress Home window cannot use this path;
    // incomplete publication must still take the lock and finish below.
    super::require_epoch(epoch)?;
    super::cleanup::require_policy(root)?;
    if admission::require_open(root).is_ok()
        && verified_mount(root, epoch)?
        && admission::require_open(root).is_ok()
    {
        return Ok(HomeResetProgress {
            reset_epoch: epoch,
            phase: HomeResetPhase::Verified,
        });
    }
    let _mutation = admission::mutation(root)?;
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
) -> Result<(), ResourceControlError> {
    let _mutation = admission::mutation(root)?;
    admission::close(root)?;
    super::cleanup::require_policy(root)?;
    if let Some(progress) = query(root)? {
        return recover_owned(root, connection, progress.reset_epoch).await;
    }
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
            verify(root.path(), &connection, 7).await,
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
        assert!(apply(root.path(), &connection, 7).await.is_err());
        assert!(verify(root.path(), &connection, 7).await.is_err());
        assert!(recover(root.path(), &connection, 7).await.is_err());
        assert!(restore_home(root.path(), &connection).await.is_err());
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
        assert!(apply(root.path(), &connection, 7).await.is_err());
        assert!(verify(root.path(), &connection, 7).await.is_err());
        assert!(recover(root.path(), &connection, 7).await.is_err());
        assert!(restore_home(root.path(), &connection).await.is_err());
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
            apply(root.path(), &connection, 7).await?;
            assert_eq!(
                verify(root.path(), &connection, 7).await?.phase,
                HomeResetPhase::Verified
            );
            recover(root.path(), &connection, 7).await?;
            restore_home(root.path(), &connection).await?;
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
        recover(root.path(), &connection, 7).await?;
        assert!(Window::read(root.path())?.is_none());
        admission::require_open(root.path())?;
        assert_eq!(
            fs::read_to_string(root.path().join("tmp/current"))?,
            "new temporary state"
        );
        // Removing or changing the image contract revokes even a current permit.
        fs::write(root.path().join("etc/natsume/contest-reset-policy"), "2\n")?;
        assert!(admission::require_open(root.path()).is_err());
        assert!(verify(root.path(), &connection, 7).await.is_err());
        Ok(())
    }
}
