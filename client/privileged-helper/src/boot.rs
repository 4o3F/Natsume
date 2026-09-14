use std::{fs, path::Path};

use natsume_local_control_api::{
    GraphicalSession, GraphicalSessionState, ResourceControlError, SessionRole,
};
use zbus::Connection;

use crate::{admission, login, session};

/// The Daemon supplies its current waiting presentation identity. Helper owns
/// the per-boot ordering fence, so a healthy service restart cannot steal focus.
pub(crate) async fn prepare(
    connection: &Connection,
    root: &Path,
    waiting: &GraphicalSession,
) -> Result<bool, ResourceControlError> {
    let boot = session::read_boot_id(root)?;
    let marker = root.join("run/natsume-privileged/boot-prepared");
    match fs::read_to_string(&marker) {
        Ok(value) if value == boot => return Ok(true),
        Ok(_) => return Err(session::rejected("boot preparation record is invalid")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(session::unavailable(
                "boot preparation record is unavailable",
            ));
        }
    }
    if login::preparing(connection).await? {
        return Ok(false);
    }
    let _mutation = admission::mutation(root)?;
    admission::require_open(root)?;
    let observed = session::observe(connection, root).await?;
    if observed.waiting.session.as_ref() != Some(waiting)
        || observed.waiting.state != GraphicalSessionState::Running
        || !observed.waiting.desktop_ready
        || observed.waiting.locked_hint
    {
        return Err(session::rejected(
            "boot preparation requires the captured ready waiting session",
        ));
    }
    match observed.contest.state {
        GraphicalSessionState::None => {
            login::start_owned(connection, root, SessionRole::Contest).await?;
            return Ok(false);
        }
        GraphicalSessionState::Starting | GraphicalSessionState::Terminating => return Ok(false),
        GraphicalSessionState::Ambiguous | GraphicalSessionState::Error => {
            return Err(session::rejected(
                "boot preparation found an invalid contest",
            ));
        }
        GraphicalSessionState::Running => {}
    }
    if !observed.contest.desktop_ready || observed.contest.locked_hint {
        if !observed.contest.locked_hint {
            login::start_owned(connection, root, SessionRole::Contest).await?;
        }
        return Ok(false);
    }
    session::activate(connection, root, SessionRole::Waiting, waiting).await?;
    let temporary = marker.with_extension("pending");
    fs::write(&temporary, boot)
        .and_then(|()| fs::rename(temporary, marker))
        .map_err(|_| session::unavailable("boot preparation cannot be recorded"))?;
    Ok(true)
}
