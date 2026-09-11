use std::{fs, path::Path};

use natsume_local_control_api::{
    GraphicalSession, GraphicalSessionObservation, GraphicalSessionState, ResourceControlError,
    SessionRole,
};
use zbus::Connection;

use crate::{admission, login, session};
use session::{rejected, unavailable};

const RECORD: &str = "run/natsume-privileged/waiting-recovery";
const PREPARATION_SECONDS: u64 = 45;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Draining,
    Preparing,
    Finished,
}

/// A single captured recovery attempt, retained across service restarts. /run
/// resets the budget only on a new boot; Finished is deliberately never removed.
#[derive(Debug, PartialEq, Eq)]
struct Recovery {
    boot: String,
    captured_id: Option<String>,
    phase: Phase,
    started: u64,
}

fn uptime(root: &Path) -> Result<u64, ResourceControlError> {
    fs::read_to_string(root.join("proc/uptime"))
        .ok()
        .and_then(|value| {
            value
                .split_whitespace()
                .next()?
                .split('.')
                .next()?
                .parse()
                .ok()
        })
        .ok_or_else(|| unavailable("boot uptime is unavailable"))
}

impl Recovery {
    fn read(root: &Path) -> Result<Option<Self>, ResourceControlError> {
        let value = match fs::read_to_string(root.join(RECORD)) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unavailable("waiting recovery record is unreadable")),
        };
        let fields: Vec<_> = value.lines().collect();
        let ["1", boot, captured, phase, started] = fields.as_slice() else {
            return Err(rejected("waiting recovery record is invalid"));
        };
        let phase = match *phase {
            "draining" => Phase::Draining,
            "preparing" => Phase::Preparing,
            "finished" => Phase::Finished,
            _ => return Err(rejected("waiting recovery phase is invalid")),
        };
        if *boot != session::read_boot_id(root)?
            || (*captured != "-"
                && (captured.is_empty() || !captured.bytes().all(|c| c.is_ascii_alphanumeric())))
        {
            return Err(rejected("waiting recovery identity is invalid"));
        }
        let started = started
            .parse::<u64>()
            .map_err(|_| rejected("waiting recovery time is invalid"))?;
        if started > uptime(root)? {
            return Err(rejected("waiting recovery time is in the future"));
        }
        Ok(Some(Self {
            boot: (*boot).to_owned(),
            captured_id: (*captured != "-").then(|| (*captured).to_owned()),
            phase,
            started,
        }))
    }

    fn captured(&self) -> Option<GraphicalSession> {
        self.captured_id.as_ref().map(|id| GraphicalSession {
            logind_session_id: id.clone(),
            boot_id: self.boot.clone(),
        })
    }

    fn persist(&self, root: &Path) -> Result<(), ResourceControlError> {
        let record = root.join(RECORD);
        let temporary = record.with_extension("pending");
        let phase = match self.phase {
            Phase::Draining => "draining",
            Phase::Preparing => "preparing",
            Phase::Finished => "finished",
        };
        // Every writer holds the checked root-only runtime's mutation lock.
        fs::write(
            &temporary,
            format!(
                "1\n{}\n{}\n{phase}\n{}\n",
                self.boot,
                self.captured_id.as_deref().unwrap_or("-"),
                self.started
            ),
        )
        .and_then(|()| fs::File::open(&temporary)?.sync_all())
        .and_then(|()| fs::rename(&temporary, &record))
        .map_err(|_| unavailable("waiting recovery record cannot be persisted"))
    }
}

pub(crate) fn require_login_allowed(root: &Path) -> Result<(), ResourceControlError> {
    if Recovery::read(root)?.is_some_and(|r| r.phase == Phase::Draining) {
        return Err(rejected("waiting drainage is still in progress"));
    }
    Ok(())
}

pub(crate) fn observe(
    root: &Path,
    waiting: &mut GraphicalSessionObservation,
) -> Result<(), ResourceControlError> {
    if let Some(record) = Recovery::read(root)?
        && record.phase == Phase::Draining
    {
        waiting.state = if waiting.session.is_none() || waiting.session == record.captured() {
            GraphicalSessionState::Terminating
        } else {
            GraphicalSessionState::Error
        };
        waiting.desktop_ready = false;
    }
    Ok(())
}

pub(crate) async fn rebuild(
    connection: &Connection,
    root: &Path,
    expected: Option<&GraphicalSession>,
) -> Result<bool, ResourceControlError> {
    let _mutation = admission::mutation(root)?;
    let mut record = if let Some(record) = Recovery::read(root)? {
        record
    } else {
        let waiting = session::observe(connection, root).await?.waiting;
        if matches!(
            waiting.state,
            GraphicalSessionState::Ambiguous | GraphicalSessionState::Error
        ) || waiting.session.as_ref() != expected
        {
            return Err(rejected("waiting recovery target is stale or ambiguous"));
        }
        let record = Recovery {
            boot: session::read_boot_id(root)?,
            captured_id: expected.map(|session| session.logind_session_id.clone()),
            phase: Phase::Draining,
            started: uptime(root)?,
        };
        record.persist(root)?;
        record
    };
    resume_owned(connection, root, &mut record).await
}

/// Service startup resumes only a previously captured attempt, never a new one.
pub(crate) async fn resume(
    connection: &Connection,
    root: &Path,
) -> Result<bool, ResourceControlError> {
    let Some(mut record) = Recovery::read(root)?.filter(|r| r.phase != Phase::Finished) else {
        return Ok(false);
    };
    let _mutation = admission::mutation(root)?;
    resume_owned(connection, root, &mut record).await
}

async fn resume_owned(
    connection: &Connection,
    root: &Path,
    record: &mut Recovery,
) -> Result<bool, ResourceControlError> {
    if record.phase == Phase::Finished {
        return Ok(false);
    }
    if record.phase == Phase::Draining {
        if session::waiting_replacement_ready(connection, root, record.captured().as_ref()).await? {
            // GDM's own waiting auto-login can win the gap between old session
            // removal and the next bounded drain retry. It has already done
            // the recreation; retain the spent budget and the new session.
            record.phase = Phase::Finished;
            record.persist(root)?;
            return Ok(false);
        }
        session::drain_waiting(connection, root, record.captured().as_ref()).await?;
        record.phase = Phase::Preparing;
        record.started = uptime(root)?;
        record.persist(root)?;
    }
    let waiting = session::observe(connection, root).await?.waiting;
    if waiting.session.is_some()
        || uptime(root)?.saturating_sub(record.started) >= PREPARATION_SECONDS
    {
        // A replacement, including one that later fails, consumes this boot's
        // attempt. It is never captured by a replay of this recovery record.
        record.phase = Phase::Finished;
        record.persist(root)?;
        return Ok(false);
    }
    login::start_owned(connection, root, SessionRole::Waiting).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{error::Error, os::unix::fs::PermissionsExt as _};

    const BOOT: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn fixture() -> Result<tempfile::TempDir, Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("run/natsume-privileged"))?;
        fs::set_permissions(
            root.path().join("run/natsume-privileged"),
            fs::Permissions::from_mode(0o700),
        )?;
        fs::create_dir_all(root.path().join("proc/sys/kernel/random"))?;
        fs::write(root.path().join("proc/sys/kernel/random/boot_id"), BOOT)?;
        fs::write(root.path().join("proc/uptime"), "100.00 0.00\n")?;
        Ok(root)
    }

    #[test]
    fn a_restarted_owner_keeps_the_capture_and_spent_budget() -> Result<(), Box<dyn Error>> {
        let root = fixture()?;
        let _mutation = admission::mutation(root.path())?;
        let mut record = Recovery {
            boot: BOOT.to_owned(),
            captured_id: Some("w1".to_owned()),
            phase: Phase::Draining,
            started: 60,
        };
        record.persist(root.path())?;
        assert_eq!(Recovery::read(root.path())?, Some(record));
        assert!(require_login_allowed(root.path()).is_err());
        record = Recovery::read(root.path())?.ok_or("missing record")?;
        record.phase = Phase::Finished;
        record.persist(root.path())?;
        assert_eq!(Recovery::read(root.path())?, Some(record));
        require_login_allowed(root.path())?;
        assert!(root.path().join(RECORD).exists());
        Ok(())
    }

    #[test]
    fn damaged_or_previous_boot_budget_is_never_reset() -> Result<(), Box<dyn Error>> {
        let root = fixture()?;
        for invalid in [
            "invalid",
            "1\nwrong-boot\nw1\nfinished\n60\n",
            "1\n550e8400-e29b-41d4-a716-446655440000\nw1\nfinished\n999\n",
        ] {
            fs::write(root.path().join(RECORD), invalid)?;
            assert!(Recovery::read(root.path()).is_err());
            assert!(require_login_allowed(root.path()).is_err());
            assert_eq!(fs::read_to_string(root.path().join(RECORD))?, invalid);
        }
        Ok(())
    }

    #[test]
    fn pending_drain_withdraws_readiness_and_rejects_a_replacement() -> Result<(), Box<dyn Error>> {
        let root = fixture()?;
        let record = Recovery {
            boot: BOOT.to_owned(),
            captured_id: Some("w1".to_owned()),
            phase: Phase::Draining,
            started: 60,
        };
        record.persist(root.path())?;
        let mut waiting = GraphicalSessionObservation {
            state: GraphicalSessionState::Running,
            session: record.captured(),
            desktop_ready: true,
            locked_hint: false,
        };
        observe(root.path(), &mut waiting)?;
        assert_eq!(waiting.state, GraphicalSessionState::Terminating);
        assert!(!waiting.desktop_ready);
        waiting.session = Some(GraphicalSession {
            boot_id: BOOT.to_owned(),
            logind_session_id: "w2".to_owned(),
        });
        observe(root.path(), &mut waiting)?;
        assert_eq!(waiting.state, GraphicalSessionState::Error);
        assert_eq!(Recovery::read(root.path())?, Some(record));
        Ok(())
    }
}
