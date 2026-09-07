use std::{fs, path::Path};

use natsume_local_control_api::{
    GraphicalSession, GraphicalSessionObservation, GraphicalSessionState,
    ManagedSessionsObservation, ResourceControlError, SessionForeground, SessionRole,
};
use uuid::Uuid;
use zbus::{Connection, Proxy, zvariant::OwnedObjectPath};

const LOGIN1_SERVICE: &str = "org.freedesktop.login1";
const LOGIN1_MANAGER_PATH: &str = "/org/freedesktop/login1";
const LOGIN1_MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const LOGIN1_SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";
const MANAGED_SEAT: &str = "seat0";
const BOOT_ID_PATH: &str = "proc/sys/kernel/random/boot_id";

struct LocalGraphicalSession {
    id: String,
    path: OwnedObjectPath,
    seat: String,
    locked: bool,
    is_x11: bool,
}

fn unavailable(message: &'static str) -> ResourceControlError {
    ResourceControlError::Unavailable(message.to_owned())
}

fn rejected(message: &'static str) -> ResourceControlError {
    ResourceControlError::Rejected(message.to_owned())
}

fn logind_error(error: zbus::Error) -> ResourceControlError {
    match zbus::fdo::Error::from(error) {
        zbus::fdo::Error::Failed(_)
        | zbus::fdo::Error::NoReply(_)
        | zbus::fdo::Error::Timeout(_)
        | zbus::fdo::Error::Disconnected(_)
        | zbus::fdo::Error::ServiceUnknown(_)
        | zbus::fdo::Error::NameHasNoOwner(_)
        | zbus::fdo::Error::IOError(_)
        | zbus::fdo::Error::NoMemory(_)
        | zbus::fdo::Error::ZBus(zbus::Error::InputOutput(_)) => {
            unavailable("logind operation is unavailable")
        }
        _ => rejected("logind operation was rejected"),
    }
}

fn read_boot_id(filesystem_root: &Path) -> Result<String, ResourceControlError> {
    let value = fs::read_to_string(filesystem_root.join(BOOT_ID_PATH))
        .map_err(|_| unavailable("boot identity is unavailable"))?;
    let value = value.trim();
    if !valid_boot_id(value) {
        return Err(rejected("boot identity is invalid"));
    }
    Ok(value.to_owned())
}

fn valid_boot_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|parsed| parsed.hyphenated().to_string() == value)
}

async fn local_graphical_sessions(
    connection: &Connection,
    role: SessionRole,
) -> Result<Vec<LocalGraphicalSession>, ResourceControlError> {
    // TODO(R1): verify the fixed UID and native GNOME runtime for each role.
    let manager = Proxy::new(
        connection,
        LOGIN1_SERVICE,
        LOGIN1_MANAGER_PATH,
        LOGIN1_MANAGER_INTERFACE,
    )
    .await
    .map_err(logind_error)?;
    let listed: Vec<(String, u32, String, String, OwnedObjectPath)> = manager
        .call("ListSessions", &())
        .await
        .map_err(logind_error)?;
    let mut sessions = Vec::new();
    for (id, _uid, user, seat, path) in listed {
        if user
            != match role {
                SessionRole::Waiting => "waiting",
                SessionRole::Contest => "contest",
            }
        {
            continue;
        }
        let session = Proxy::new(
            connection,
            LOGIN1_SERVICE,
            path.as_str(),
            LOGIN1_SESSION_INTERFACE,
        )
        .await
        .map_err(logind_error)?;
        let remote: bool = session.get_property("Remote").await.map_err(logind_error)?;
        let class: String = session.get_property("Class").await.map_err(logind_error)?;
        let kind: String = session.get_property("Type").await.map_err(logind_error)?;
        if remote || class != "user" || !matches!(kind.as_str(), "x11" | "wayland") {
            continue;
        }
        let locked = session
            .get_property("LockedHint")
            .await
            .map_err(logind_error)?;
        sessions.push(LocalGraphicalSession {
            id,
            path,
            seat,
            locked,
            is_x11: kind == "x11",
        });
    }
    Ok(sessions)
}

pub(super) async fn observe(
    connection: &Connection,
    filesystem_root: &Path,
) -> Result<ManagedSessionsObservation, ResourceControlError> {
    let boot_id = read_boot_id(filesystem_root)?;
    let waiting = local_graphical_sessions(connection, SessionRole::Waiting).await?;
    let contest = local_graphical_sessions(connection, SessionRole::Contest).await?;
    Ok(ManagedSessionsObservation {
        waiting: observe_sessions(&waiting, boot_id.clone()),
        contest: observe_sessions(&contest, boot_id),
        // TODO(R1): sample seat0's actual foreground independently of role lifecycle.
        foreground: SessionForeground::Unknown,
    })
}

fn observe_sessions(
    sessions: &[LocalGraphicalSession],
    boot_id: String,
) -> GraphicalSessionObservation {
    let (state, session, locked_hint) = match sessions {
        [] => (GraphicalSessionState::None, None, false),
        [session] if session.seat == MANAGED_SEAT && session.is_x11 => (
            GraphicalSessionState::Running,
            Some(GraphicalSession {
                logind_session_id: session.id.clone(),
                boot_id,
            }),
            session.locked,
        ),
        _ => (GraphicalSessionState::Ambiguous, None, false),
    };
    GraphicalSessionObservation {
        state,
        session,
        // TODO(R1): verify the native GNOME desktop; a logind session is insufficient.
        desktop_ready: false,
        locked_hint,
    }
}

fn exact_termination_path(
    sessions: &[LocalGraphicalSession],
    target_id: &str,
) -> Result<Option<OwnedObjectPath>, ResourceControlError> {
    let mut matches = sessions.iter().filter(|session| session.id == target_id);
    let target = matches.next();
    if matches.next().is_some() {
        return Err(rejected("graphical session target is not unique"));
    }
    Ok(target.map(|target| target.path.clone()))
}

pub(super) async fn terminate(
    connection: &Connection,
    filesystem_root: &Path,
    target: &GraphicalSession,
) -> Result<(), ResourceControlError> {
    if !valid_boot_id(&target.boot_id) {
        return Err(rejected("graphical session target is invalid"));
    }
    if read_boot_id(filesystem_root)? != target.boot_id {
        return Ok(());
    }
    let sessions = local_graphical_sessions(connection, SessionRole::Contest).await?;
    let Some(path) = exact_termination_path(&sessions, &target.logind_session_id)? else {
        return Ok(());
    };
    let session = Proxy::new(connection, LOGIN1_SERVICE, path, LOGIN1_SESSION_INTERFACE)
        .await
        .map_err(logind_error)?;
    let result: Result<(), _> = session.call("Terminate", &()).await;
    result.map_err(logind_error)?;
    let remaining = local_graphical_sessions(connection, SessionRole::Contest).await?;
    if exact_termination_path(&remaining, &target.logind_session_id)?.is_some() {
        return Err(unavailable("logind session termination is incomplete"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use zbus::zvariant::OwnedObjectPath;

    use natsume_local_control_api::GraphicalSessionState;

    use super::{
        LocalGraphicalSession, exact_termination_path, observe_sessions, read_boot_id,
        valid_boot_id,
    };

    fn candidate(id: &str, path: &str, seat: &str) -> LocalGraphicalSession {
        let path = OwnedObjectPath::try_from(path)
            .unwrap_or_else(|error| panic!("fixture object path failed: {error}"));
        LocalGraphicalSession {
            id: id.to_owned(),
            path,
            seat: seat.to_owned(),
            locked: false,
            is_x11: true,
        }
    }

    #[test]
    fn logind_availability_and_safety_errors_stay_distinct() {
        assert!(matches!(
            super::logind_error(zbus::fdo::Error::NoReply("timeout".to_owned()).into()),
            natsume_local_control_api::ResourceControlError::Unavailable(_)
        ));
        assert!(matches!(
            super::logind_error(zbus::fdo::Error::AccessDenied("denied".to_owned()).into()),
            natsume_local_control_api::ResourceControlError::Rejected(_)
        ));
        assert!(matches!(
            super::logind_error(zbus::Error::Failure("unknown".to_owned())),
            natsume_local_control_api::ResourceControlError::Rejected(_)
        ));
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

        assert!(matches!(
            read_boot_id(fixture.path()).as_deref(),
            Ok("550e8400-e29b-41d4-a716-446655440000")
        ));

        fs::write(
            path.join("boot_id"),
            "550E8400-E29B-41D4-A716-446655440000\n",
        )
        .unwrap_or_else(|error| panic!("fixture rewrite failed: {error}"));
        assert!(matches!(
            read_boot_id(fixture.path()),
            Err(natsume_local_control_api::ResourceControlError::Rejected(_))
        ));
        assert!(!valid_boot_id("not-a-boot-id"));
    }

    #[test]
    fn exact_session_never_retargets_or_selects_an_ambiguous_candidate() {
        let replacement = [candidate(
            "c3",
            "/org/freedesktop/login1/session/c3",
            "seat0",
        )];
        assert_eq!(
            exact_termination_path(&replacement, "c2")
                .unwrap_or_else(|error| panic!("absent session lookup failed: {error}")),
            None
        );

        let ambiguous = [
            candidate("c2", "/org/freedesktop/login1/session/c2", "seat0"),
            candidate("c3", "/org/freedesktop/login1/session/c3", "seat0"),
        ];
        assert_eq!(
            exact_termination_path(&ambiguous, "c2")
                .unwrap_or_else(|error| panic!("old session lookup failed: {error}"))
                .unwrap_or_else(|| panic!("old session must be found"))
                .as_str(),
            "/org/freedesktop/login1/session/c2"
        );
    }

    #[test]
    fn a_background_session_is_running_but_another_seat_is_ambiguous() {
        let boot_id = "550e8400-e29b-41d4-a716-446655440000".to_owned();
        let inactive = [candidate(
            "c2",
            "/org/freedesktop/login1/session/c2",
            "seat0",
        )];
        assert_eq!(
            observe_sessions(&inactive, boot_id.clone()).state,
            GraphicalSessionState::Running
        );

        let other_seat = [candidate(
            "c3",
            "/org/freedesktop/login1/session/c3",
            "seat1",
        )];
        assert_eq!(
            observe_sessions(&other_seat, boot_id).state,
            GraphicalSessionState::Ambiguous
        );
    }

    #[test]
    fn lock_hint_and_session_presence_cannot_claim_desktop_readiness() {
        let mut session = candidate("c2", "/org/freedesktop/login1/session/c2", "seat0");
        session.locked = true;
        let observed = observe_sessions(
            &[session],
            "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        );
        assert_eq!(observed.state, GraphicalSessionState::Running);
        assert!(observed.locked_hint);
        assert!(!observed.desktop_ready);
        assert!(observed.session.is_some());
    }

    #[test]
    fn unsupported_graphics_and_multiple_candidates_are_not_absent_or_running() {
        let boot = "550e8400-e29b-41d4-a716-446655440000".to_owned();
        let mut unsupported = candidate("c2", "/org/freedesktop/login1/session/c2", "seat0");
        unsupported.is_x11 = false;
        assert_eq!(
            observe_sessions(&[unsupported], boot.clone()).state,
            GraphicalSessionState::Ambiguous
        );
        let multiple = [
            candidate("c2", "/org/freedesktop/login1/session/c2", "seat0"),
            candidate("c3", "/org/freedesktop/login1/session/c3", "seat0"),
        ];
        let observed = observe_sessions(&multiple, boot);
        assert_eq!(observed.state, GraphicalSessionState::Ambiguous);
        assert!(observed.session.is_none());
        assert!(!observed.desktop_ready);
    }
}
