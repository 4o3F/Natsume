use std::{fs, path::Path};

use natsume_local_control_api::{
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
    }
}

pub(crate) fn query(root: &Path) -> Result<Option<HomeResetProgress>, ResourceControlError> {
    let progress = super::query(root)?;
    if let Some(window) = Window::read(root)? {
        if let Some(progress) = progress.filter(|progress| progress.reset_epoch == window.epoch) {
            return Ok(Some(progress));
        }
        return Ok(Some(HomeResetProgress {
            reset_epoch: window.epoch,
            phase: HomeResetPhase::Draining,
        }));
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
    Ok(guard)
}

fn require_epoch(root: &Path, epoch: u64) -> Result<Option<Window>, ResourceControlError> {
    super::require_epoch(epoch)?;
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
        super::require_target_progress(root, epoch)?;
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

pub(crate) fn verify(root: &Path, epoch: u64) -> Result<HomeResetProgress, ResourceControlError> {
    // GDM preparation holds the mutation lock while it waits for the desktop.
    // Observing a completed, still mounted generation needs no mutation. Check
    // admission on both sides so an in-progress Home window cannot use this path;
    // incomplete publication must still take the lock and finish below.
    super::require_epoch(epoch)?;
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
    require_epoch(root, epoch)?;
    if query(root)?.is_some_and(|p| p.reset_epoch == epoch && p.phase == HomeResetPhase::Draining) {
        return Ok(HomeResetProgress {
            reset_epoch: epoch,
            phase: HomeResetPhase::Draining,
        });
    }
    let progress = super::verify(root, epoch)?;
    if progress.phase == HomeResetPhase::Verified {
        finish(root, epoch)?;
    } else {
        admission::close(root)?;
    }
    Ok(progress)
}

fn finish(root: &Path, epoch: u64) -> Result<(), ResourceControlError> {
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
    use super::*;
    use std::{error::Error, os::unix::fs::PermissionsExt as _};

    const BOOT: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn fixture() -> Result<tempfile::TempDir, Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(super::super::state_directory(root.path()))?;
        let runtime = root.path().join("run/natsume-privileged");
        fs::create_dir_all(&runtime)?;
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;
        fs::create_dir_all(root.path().join("proc/sys/kernel/random"))?;
        fs::write(root.path().join("proc/sys/kernel/random/boot_id"), BOOT)?;
        Ok(root)
    }

    #[test]
    fn durable_window_withdraws_even_a_previously_published_permit() -> Result<(), Box<dyn Error>> {
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
    fn invalid_windows_stay_closed_and_are_not_rewritten() -> Result<(), Box<dyn Error>> {
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

    #[test]
    fn a_verified_marker_without_a_mount_is_not_ready_while_mutation_is_busy()
    -> Result<(), Box<dyn Error>> {
        let root = fixture()?;
        fs::create_dir_all(root.path().join("proc/self"))?;
        fs::write(root.path().join("proc/self/mountinfo"), "")?;
        super::super::write_progress(root.path(), 7, HomeResetPhase::Verified)?;
        admission::open(root.path())?;
        let _mutation = admission::mutation(root.path())?;

        assert!(matches!(
            verify(root.path(), 7),
            Err(ResourceControlError::Unavailable(_))
        ));
        assert_eq!(
            super::super::query(root.path())?.map(|progress| progress.phase),
            Some(HomeResetPhase::Verified)
        );
        admission::require_open(root.path())?;
        Ok(())
    }
}
