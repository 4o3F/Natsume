use std::{fs, path::Path};

use natsume_local_control_api::{
    ContestSessionObservation, ContestSessionState, GraphicalSession, SessionLockLevel,
};
use uuid::Uuid;
use zbus::{Connection, Proxy, zvariant::OwnedObjectPath};

const LOGIN1_SERVICE: &str = "org.freedesktop.login1";
const LOGIN1_MANAGER_PATH: &str = "/org/freedesktop/login1";
const LOGIN1_MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const LOGIN1_SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";
const CONTEST_USER: &str = "contest";
const CONTEST_SEAT: &str = "seat0";
const BOOT_ID_PATH: &str = "proc/sys/kernel/random/boot_id";

struct LocalGraphicalSession {
    id: String,
    path: OwnedObjectPath,
    active: bool,
    seat: String,
    locked: bool,
}

fn service_error(message: &'static str) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(message.to_owned())
}

fn read_boot_id(filesystem_root: &Path) -> zbus::fdo::Result<String> {
    let value = fs::read_to_string(filesystem_root.join(BOOT_ID_PATH))
        .map_err(|_| service_error("boot identity is unavailable"))?;
    let value = value.trim();
    if !valid_boot_id(value) {
        return Err(service_error("boot identity is invalid"));
    }
    Ok(value.to_owned())
}

fn valid_boot_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|parsed| parsed.hyphenated().to_string() == value)
}

async fn local_graphical_sessions(
    connection: &Connection,
) -> zbus::fdo::Result<Vec<LocalGraphicalSession>> {
    let manager = Proxy::new(
        connection,
        LOGIN1_SERVICE,
        LOGIN1_MANAGER_PATH,
        LOGIN1_MANAGER_INTERFACE,
    )
    .await
    .map_err(|_| service_error("logind is unavailable"))?;
    let listed: Vec<(String, u32, String, String, OwnedObjectPath)> = manager
        .call("ListSessions", &())
        .await
        .map_err(|_| service_error("logind session query failed"))?;
    let mut sessions = Vec::new();
    for (id, _uid, user, seat, path) in listed {
        if user != CONTEST_USER {
            continue;
        }
        let session = Proxy::new(
            connection,
            LOGIN1_SERVICE,
            path.as_str(),
            LOGIN1_SESSION_INTERFACE,
        )
        .await
        .map_err(|_| service_error("logind session query failed"))?;
        let active: bool = session
            .get_property("Active")
            .await
            .map_err(|_| service_error("logind session query failed"))?;
        let remote: bool = session
            .get_property("Remote")
            .await
            .map_err(|_| service_error("logind session query failed"))?;
        let class: String = session
            .get_property("Class")
            .await
            .map_err(|_| service_error("logind session query failed"))?;
        let kind: String = session
            .get_property("Type")
            .await
            .map_err(|_| service_error("logind session query failed"))?;
        if remote || class != "user" || !matches!(kind.as_str(), "x11" | "wayland") {
            continue;
        }
        let locked = session
            .get_property("LockedHint")
            .await
            .map_err(|_| service_error("logind session query failed"))?;
        sessions.push(LocalGraphicalSession {
            id,
            path,
            active,
            seat,
            locked,
        });
    }
    Ok(sessions)
}

pub(super) async fn observe(
    connection: &Connection,
    filesystem_root: &Path,
) -> zbus::fdo::Result<ContestSessionObservation> {
    let boot_id = read_boot_id(filesystem_root)?;
    let sessions = local_graphical_sessions(connection).await?;
    Ok(observe_sessions(&sessions, boot_id))
}

fn observe_sessions(
    sessions: &[LocalGraphicalSession],
    boot_id: String,
) -> ContestSessionObservation {
    match sessions {
        [] => ContestSessionObservation {
            state: ContestSessionState::None,
            session: None,
        },
        [session] if session.active && session.seat == CONTEST_SEAT => ContestSessionObservation {
            state: if session.locked {
                ContestSessionState::Locked
            } else {
                ContestSessionState::Active
            },
            session: Some(GraphicalSession {
                logind_session_id: session.id.clone(),
                boot_id,
            }),
        },
        _ => ContestSessionObservation {
            state: ContestSessionState::Ambiguous,
            session: None,
        },
    }
}

async fn exact_lock_session<'a>(
    connection: &'a Connection,
    filesystem_root: &Path,
    target: &GraphicalSession,
) -> zbus::fdo::Result<Proxy<'a>> {
    if read_boot_id(filesystem_root)? != target.boot_id {
        return Err(service_error("graphical session target is stale"));
    }
    let sessions = local_graphical_sessions(connection).await?;
    let path = exact_active_session_path(&sessions, &target.logind_session_id)?;
    Proxy::new(connection, LOGIN1_SERVICE, path, LOGIN1_SESSION_INTERFACE)
        .await
        .map_err(|_| service_error("logind session query failed"))
}

fn exact_active_session_path(
    sessions: &[LocalGraphicalSession],
    target_id: &str,
) -> zbus::fdo::Result<OwnedObjectPath> {
    let [session] = sessions else {
        return Err(service_error("graphical session target is not unique"));
    };
    if session.id != target_id || !session.active || session.seat != CONTEST_SEAT {
        return Err(service_error("graphical session target is stale"));
    }
    Ok(session.path.clone())
}

fn exact_termination_path(
    sessions: &[LocalGraphicalSession],
    target_id: &str,
) -> zbus::fdo::Result<Option<OwnedObjectPath>> {
    let mut matches = sessions.iter().filter(|session| session.id == target_id);
    let target = matches.next();
    if matches.next().is_some() {
        return Err(service_error("graphical session target is not unique"));
    }
    Ok(target.map(|target| target.path.clone()))
}

pub(super) async fn set_lock(
    connection: &Connection,
    filesystem_root: &Path,
    target: &GraphicalSession,
    level: SessionLockLevel,
) -> zbus::fdo::Result<()> {
    let session = exact_lock_session(connection, filesystem_root, target).await?;
    let method = match level {
        SessionLockLevel::Unlocked => "Unlock",
        SessionLockLevel::Locked => "Lock",
    };
    let result: Result<(), _> = session.call(method, &()).await;
    result.map_err(|_| service_error("logind lock transition failed"))
}

pub(super) async fn terminate(
    connection: &Connection,
    filesystem_root: &Path,
    target: &GraphicalSession,
) -> zbus::fdo::Result<()> {
    if !valid_boot_id(&target.boot_id) {
        return Err(service_error("graphical session target is invalid"));
    }
    if read_boot_id(filesystem_root)? != target.boot_id {
        return Ok(());
    }
    let sessions = local_graphical_sessions(connection).await?;
    let Some(path) = exact_termination_path(&sessions, &target.logind_session_id)? else {
        return Ok(());
    };
    let session = Proxy::new(connection, LOGIN1_SERVICE, path, LOGIN1_SESSION_INTERFACE)
        .await
        .map_err(|_| service_error("logind session query failed"))?;
    let result: Result<(), _> = session.call("Terminate", &()).await;
    result.map_err(|_| service_error("logind session termination failed"))?;
    let remaining = local_graphical_sessions(connection).await?;
    if exact_termination_path(&remaining, &target.logind_session_id)?.is_some() {
        return Err(service_error("logind session termination is incomplete"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use zbus::zvariant::OwnedObjectPath;

    use natsume_local_control_api::ContestSessionState;

    use super::{
        LocalGraphicalSession, exact_active_session_path, exact_termination_path, observe_sessions,
        read_boot_id, valid_boot_id,
    };

    fn candidate(id: &str, path: &str, active: bool, seat: &str) -> LocalGraphicalSession {
        let path = OwnedObjectPath::try_from(path)
            .unwrap_or_else(|error| panic!("fixture object path failed: {error}"));
        LocalGraphicalSession {
            id: id.to_owned(),
            path,
            active,
            seat: seat.to_owned(),
            locked: false,
        }
    }

    #[test]
    fn boot_id_requires_canonical_lowercase_uuid() {
        let fixture = TempDir::new().unwrap_or_else(|error| panic!("fixture failed: {error}"));
        let path = fixture.path().join("proc/sys/kernel/random");
        fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("fixture directory failed: {error}"));
        fs::write(
            path.join("boot_id"),
            "550e8400-e29b-41d4-a716-446655440000\n",
        )
        .unwrap_or_else(|error| panic!("fixture write failed: {error}"));

        assert_eq!(
            read_boot_id(fixture.path()).as_deref(),
            Ok("550e8400-e29b-41d4-a716-446655440000")
        );

        fs::write(
            path.join("boot_id"),
            "550E8400-E29B-41D4-A716-446655440000\n",
        )
        .unwrap_or_else(|error| panic!("fixture rewrite failed: {error}"));
        assert!(read_boot_id(fixture.path()).is_err());
        assert!(!valid_boot_id("not-a-boot-id"));
    }

    #[test]
    fn exact_session_never_retargets_or_selects_an_ambiguous_candidate() {
        let replacement = [candidate(
            "c3",
            "/org/freedesktop/login1/session/c3",
            true,
            "seat0",
        )];
        assert!(exact_active_session_path(&replacement, "c2").is_err());
        assert_eq!(
            exact_termination_path(&replacement, "c2")
                .unwrap_or_else(|error| panic!("absent session lookup failed: {error}")),
            None
        );

        let ambiguous = [
            candidate("c2", "/org/freedesktop/login1/session/c2", true, "seat0"),
            candidate("c3", "/org/freedesktop/login1/session/c3", true, "seat0"),
        ];
        assert!(exact_active_session_path(&ambiguous, "c2").is_err());
        assert_eq!(
            exact_termination_path(&ambiguous, "c2")
                .unwrap_or_else(|error| panic!("old session lookup failed: {error}"))
                .unwrap_or_else(|| panic!("old session must be found"))
                .as_str(),
            "/org/freedesktop/login1/session/c2"
        );
    }

    #[test]
    fn inactive_or_non_contest_seat_sessions_are_never_absent() {
        let boot_id = "550e8400-e29b-41d4-a716-446655440000".to_owned();
        let inactive = [candidate(
            "c2",
            "/org/freedesktop/login1/session/c2",
            false,
            "seat0",
        )];
        assert_eq!(
            observe_sessions(&inactive, boot_id.clone()).state,
            ContestSessionState::Ambiguous
        );

        let other_seat = [candidate(
            "c3",
            "/org/freedesktop/login1/session/c3",
            true,
            "seat1",
        )];
        assert_eq!(
            observe_sessions(&other_seat, boot_id).state,
            ContestSessionState::Ambiguous
        );
    }
}
