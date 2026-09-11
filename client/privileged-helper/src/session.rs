use std::{fs, path::Path};

use natsume_local_control_api::{
    GraphicalSession, GraphicalSessionObservation, GraphicalSessionState,
    ManagedSessionsObservation, ResourceControlError, SessionForeground, SessionRole,
};
use tokio::time::{Duration, timeout};
use uuid::Uuid;
use zbus::{Connection, Proxy, proxy::CacheProperties, zvariant::OwnedObjectPath};

const LOGIN1_SERVICE: &str = "org.freedesktop.login1";
const LOGIN1_MANAGER_PATH: &str = "/org/freedesktop/login1";
const LOGIN1_MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const LOGIN1_SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";
const MANAGED_SEAT: &str = "seat0";
const BOOT_ID_PATH: &str = "proc/sys/kernel/random/boot_id";
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(5);

type ListedSession = (String, u32, String, String, OwnedObjectPath);

pub(crate) struct Account {
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

struct LocalGraphicalSession {
    id: String,
    path: OwnedObjectPath,
    seat: String,
    locked: bool,
    is_x11: bool,
    state: GraphicalSessionState,
}

pub(crate) fn unavailable(message: &'static str) -> ResourceControlError {
    ResourceControlError::Unavailable(message.to_owned())
}

pub(crate) fn rejected(message: &'static str) -> ResourceControlError {
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

pub(crate) fn read_boot_id(filesystem_root: &Path) -> Result<String, ResourceControlError> {
    let value = fs::read_to_string(filesystem_root.join(BOOT_ID_PATH))
        .map_err(|_| unavailable("boot identity is unavailable"))?;
    let value = value.trim();
    if !valid_boot_id(value) {
        return Err(rejected("boot identity is invalid"));
    }
    Ok(value.to_owned())
}

pub(crate) fn valid_boot_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|parsed| parsed.hyphenated().to_string() == value)
}

pub(crate) const fn role_name(role: SessionRole) -> &'static str {
    match role {
        SessionRole::Waiting => "waiting",
        SessionRole::Contest => "contest",
    }
}

/// Unix account names are independent of the fixed role/CLI tokens.
pub(crate) const fn account_name(role: SessionRole) -> &'static str {
    match role {
        SessionRole::Waiting => "waiting",
        SessionRole::Contest => "teams",
    }
}

pub(crate) fn account(root: &Path, name: &str) -> Result<Account, ResourceControlError> {
    let passwd = fs::read_to_string(root.join("etc/passwd"))
        .map_err(|_| unavailable("fixed graphical accounts are unavailable"))?;
    let rows: Vec<Vec<&str>> = passwd
        .lines()
        .map(|line| line.split(':').collect())
        .collect();
    let matching: Vec<_> = rows
        .iter()
        .filter(|row| row.first() == Some(&name))
        .collect();
    let [row] = matching.as_slice() else {
        return Err(rejected("fixed graphical account is missing or ambiguous"));
    };
    if row.len() != 7 {
        return Err(rejected("fixed graphical account is invalid"));
    }
    let uid = row.get(2).and_then(|value| value.parse::<u32>().ok());
    let gid = row.get(3).and_then(|value| value.parse::<u32>().ok());
    let (Some(uid), Some(gid)) = (uid, gid) else {
        return Err(rejected("fixed graphical account is invalid"));
    };
    if uid == 0
        || rows
            .iter()
            .filter(|row| row.get(2).and_then(|value| value.parse::<u32>().ok()) == Some(uid))
            .count()
            != 1
    {
        return Err(rejected("fixed graphical UID is not unique"));
    }
    Ok(Account { uid, gid })
}

pub(crate) async fn fresh_proxy<'a>(
    connection: &'a Connection,
    destination: &'a str,
    path: &'a str,
    interface: &'a str,
) -> zbus::Result<Proxy<'a>> {
    zbus::proxy::Builder::<Proxy<'_>>::new(connection)
        .destination(destination)?
        .path(path)?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

async fn listed_sessions(
    connection: &Connection,
) -> Result<Vec<ListedSession>, ResourceControlError> {
    fresh_proxy(
        connection,
        LOGIN1_SERVICE,
        LOGIN1_MANAGER_PATH,
        LOGIN1_MANAGER_INTERFACE,
    )
    .await
    .map_err(logind_error)?
    .call("ListSessions", &())
    .await
    .map_err(logind_error)
}

async fn local_graphical_sessions(
    connection: &Connection,
    root: &Path,
    role: SessionRole,
) -> Result<Vec<LocalGraphicalSession>, ResourceControlError> {
    let expected = account(root, account_name(role))?;
    let mut sessions = Vec::new();
    for (id, uid, user, seat, path) in listed_sessions(connection).await? {
        if uid != expected.uid && user != account_name(role) {
            continue;
        }
        let session = fresh_proxy(
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
        let state: String = session.get_property("State").await.map_err(logind_error)?;
        let state = match state.as_str() {
            "opening" => GraphicalSessionState::Starting,
            "active" | "online" => GraphicalSessionState::Running,
            "closing" => GraphicalSessionState::Terminating,
            _ => GraphicalSessionState::Error,
        };
        sessions.push(LocalGraphicalSession {
            id,
            path,
            seat,
            locked,
            state,
            is_x11: kind == "x11" && uid == expected.uid && user == account_name(role),
        });
    }
    Ok(sessions)
}

async fn active_session(connection: &Connection) -> Result<String, ResourceControlError> {
    let manager = fresh_proxy(
        connection,
        LOGIN1_SERVICE,
        LOGIN1_MANAGER_PATH,
        LOGIN1_MANAGER_INTERFACE,
    )
    .await
    .map_err(logind_error)?;
    let path: OwnedObjectPath = manager
        .call("GetSeat", &(MANAGED_SEAT,))
        .await
        .map_err(logind_error)?;
    let seat = fresh_proxy(
        connection,
        LOGIN1_SERVICE,
        path.as_str(),
        "org.freedesktop.login1.Seat",
    )
    .await
    .map_err(logind_error)?;
    let (id, _): (String, OwnedObjectPath) = seat
        .get_property("ActiveSession")
        .await
        .map_err(logind_error)?;
    Ok(id)
}

pub(crate) async fn contest_absent(
    connection: &Connection,
    root: &Path,
) -> Result<bool, ResourceControlError> {
    Ok(
        local_graphical_sessions(connection, root, SessionRole::Contest)
            .await?
            .is_empty(),
    )
}

async fn desktop_ready(
    connection: &Connection,
    root: &Path,
    role: SessionRole,
    session: &GraphicalSession,
) -> bool {
    let operation = async {
        let expected = account(root, account_name(role))
            .map_err(|_| zbus::Error::Failure("account unavailable".to_owned()))?;
        let manager = fresh_proxy(
            connection,
            LOGIN1_SERVICE,
            LOGIN1_MANAGER_PATH,
            LOGIN1_MANAGER_INTERFACE,
        )
        .await?;
        let path: OwnedObjectPath = manager.call("GetUser", &(expected.uid,)).await?;
        let user = fresh_proxy(
            connection,
            LOGIN1_SERVICE,
            path.as_str(),
            "org.freedesktop.login1.User",
        )
        .await?;
        let (user_display, _): (String, OwnedObjectPath) = user.get_property("Display").await?;
        if user_display != session.logind_session_id {
            return Ok(false);
        }
        let active_before = active_session(connection)
            .await
            .map_err(|_| zbus::Error::Failure("foreground unavailable".to_owned()))?;
        let output = tokio::process::Command::new("/usr/bin/setpriv")
            .args([
                "--reuid",
                &expected.uid.to_string(),
                "--regid",
                &expected.gid.to_string(),
                "--clear-groups",
                "--no-new-privs",
                "/usr/lib/natsume/natsume-privileged-helper",
                "desktop-status",
                role_name(role),
            ])
            .env_clear()
            .env(
                "XAUTHORITY",
                format!("/run/user/{}/gdm/Xauthority", expected.uid),
            )
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|_| zbus::Error::Failure("desktop probe unavailable".to_owned()))?;
        let active_after = active_session(connection)
            .await
            .map_err(|_| zbus::Error::Failure("foreground unavailable".to_owned()))?;
        // A delayed Xorg VT entry can leave its cached outputs ready while it
        // believes it owns a background VT. Do not activate that inconsistent
        // desktop: the next acquire signal can be mistaken for a VT release.
        let expected_vt: &[u8] = if active_before == session.logind_session_id {
            b"true\n"
        } else {
            b"false\n"
        };
        Ok::<_, zbus::Error>(
            output.status.success()
                && output.stdout == expected_vt
                && active_before == active_after,
        )
    };
    matches!(
        timeout(Duration::from_secs(2), operation).await,
        Ok(Ok(true))
    )
}

/// Queries the fixed role's GNOME bus and Xorg under that role's Unix identity.
/// Returns Xorg's VT observation for the parent to compare with logind.
///
/// # Errors
/// Rejects wrong users, missing GNOME owners and an incomplete desktop startup.
pub async fn probe_desktop(role: SessionRole) -> Result<bool, ResourceControlError> {
    let expected = account(Path::new("/"), account_name(role))?;
    if rustix::process::getuid().as_raw() != expected.uid
        || rustix::process::geteuid().as_raw() != expected.uid
    {
        return Err(rejected("desktop probe requires its fixed role identity"));
    }
    if std::env::var_os("XAUTHORITY")
        != Some(format!("/run/user/{}/gdm/Xauthority", expected.uid).into())
    {
        return Err(rejected("desktop probe requires its fixed GDM authority"));
    }
    let operation = async {
        // Use the fixed UID runtime, never the caller's DISPLAY, environment or address.
        let address = format!("unix:path=/run/user/{}/bus", expected.uid);
        let bus = zbus::connection::Builder::address(address.as_str())?
            .method_timeout(Duration::from_secs(1))
            .build()
            .await?;
        let dbus = zbus::fdo::DBusProxy::new(&bus).await?;
        let compositor = match role {
            SessionRole::Waiting => "org.gnome.Kiosk",
            SessionRole::Contest => "org.gnome.Shell",
        };
        for name in ["org.gnome.SessionManager", compositor] {
            let owner = dbus.get_name_owner(name.try_into()?).await?;
            if dbus.get_connection_unix_user(owner.into()).await? != expected.uid {
                return Ok(None);
            }
        }
        let gnome = fresh_proxy(
            &bus,
            "org.gnome.SessionManager",
            "/org/gnome/SessionManager",
            "org.gnome.SessionManager",
        )
        .await?;
        if !gnome.call::<_, _, bool>("IsSessionRunning", &()).await? {
            return Ok(None);
        }
        let owner = dbus.get_name_owner(compositor.try_into()?).await?;
        let pid = dbus.get_connection_unix_process_id(owner.into()).await?;
        // GDM leaves logind's Display empty on the supported image. Read only
        // DISPLAY from the live compositor owner, under its own UID; never
        // inherit a root caller's display or user-manager environment.
        let display = i32::try_from(pid)
            .ok()
            .and_then(|pid| procfs::process::Process::new(pid).ok())
            .and_then(|process| process.environ().ok())
            .and_then(|environment| environment.get(std::ffi::OsStr::new("DISPLAY")).cloned())
            .and_then(|display| display.into_string().ok());
        Ok::<_, zbus::Error>(display)
    };
    let Ok(Ok(Some(display))) = timeout(Duration::from_secs(1), operation).await else {
        return Err(unavailable("GNOME desktop is not running"));
    };
    // Synchronous X11 I/O stays in this disposable UID child. The parent kills
    // it after two seconds, including a stalled connection or reply.
    crate::display::probe(&display)
}

pub(crate) async fn observe(
    connection: &Connection,
    root: &Path,
) -> Result<ManagedSessionsObservation, ResourceControlError> {
    timeout(OBSERVATION_TIMEOUT, observe_inner(connection, root))
        .await
        .map_err(|_| unavailable("graphical session observation timed out"))?
}

async fn observe_inner(
    connection: &Connection,
    root: &Path,
) -> Result<ManagedSessionsObservation, ResourceControlError> {
    let boot = read_boot_id(root)?;
    let waiting = local_graphical_sessions(connection, root, SessionRole::Waiting).await?;
    let contest = local_graphical_sessions(connection, root, SessionRole::Contest).await?;
    let mut waiting = observe_sessions(&waiting, boot.clone());
    let mut contest = observe_sessions(&contest, boot);
    for (role, observation) in [
        (SessionRole::Waiting, &mut waiting),
        (SessionRole::Contest, &mut contest),
    ] {
        if observation.state == GraphicalSessionState::Running
            && let Some(session) = observation.session.as_ref()
        {
            observation.desktop_ready = desktop_ready(connection, root, role, session).await;
        }
    }
    crate::waiting::observe(root, &mut waiting)?;
    let active = active_session(connection).await?;
    let foreground = if active.is_empty() {
        SessionForeground::None
    } else if waiting
        .session
        .as_ref()
        .is_some_and(|s| s.logind_session_id == active)
    {
        SessionForeground::Waiting
    } else if contest
        .session
        .as_ref()
        .is_some_and(|s| s.logind_session_id == active)
    {
        SessionForeground::Contest
    } else {
        let greeter = greeter(connection, root).await?;
        if greeter.as_ref().is_some_and(|(id, _)| *id == active) {
            SessionForeground::Greeter
        } else {
            SessionForeground::Other
        }
    };
    Ok(ManagedSessionsObservation {
        waiting,
        contest,
        foreground,
    })
}

fn observe_sessions(
    sessions: &[LocalGraphicalSession],
    boot_id: String,
) -> GraphicalSessionObservation {
    let (state, session, locked_hint) = match sessions {
        [] => (GraphicalSessionState::None, None, false),
        [session] if session.seat == MANAGED_SEAT && session.is_x11 => (
            session.state,
            matches!(
                session.state,
                GraphicalSessionState::Starting
                    | GraphicalSessionState::Running
                    | GraphicalSessionState::Terminating
            )
            .then(|| GraphicalSession {
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
        desktop_ready: false,
        locked_hint,
    }
}

pub(crate) async fn greeter(
    connection: &Connection,
    root: &Path,
) -> Result<Option<(String, OwnedObjectPath)>, ResourceControlError> {
    let gdm = account(root, "gdm")?;
    let mut candidates = Vec::new();
    for (id, uid, _, seat, path) in listed_sessions(connection).await? {
        if uid != gdm.uid || seat != MANAGED_SEAT {
            continue;
        }
        let session = fresh_proxy(
            connection,
            LOGIN1_SERVICE,
            path.as_str(),
            LOGIN1_SESSION_INTERFACE,
        )
        .await
        .map_err(logind_error)?;
        let class: String = session.get_property("Class").await.map_err(logind_error)?;
        let state: String = session.get_property("State").await.map_err(logind_error)?;
        let remote: bool = session.get_property("Remote").await.map_err(logind_error)?;
        if class == "greeter" && state != "closing" && !remote {
            candidates.push((id, path));
        }
    }
    if candidates.len() > 1 {
        return Err(rejected("GDM greeter is ambiguous"));
    }
    Ok(candidates.pop())
}

pub(crate) async fn activate_greeter(
    connection: &Connection,
    id: &str,
    path: &OwnedObjectPath,
) -> Result<(), ResourceControlError> {
    fresh_proxy(
        connection,
        LOGIN1_SERVICE,
        path.as_str(),
        LOGIN1_SESSION_INTERFACE,
    )
    .await
    .map_err(logind_error)?
    .call::<_, _, ()>("Activate", &())
    .await
    .map_err(logind_error)?;
    wait_for_foreground(connection, id).await
}

async fn wait_for_foreground(
    connection: &Connection,
    id: &str,
) -> Result<(), ResourceControlError> {
    // logind can acknowledge Activate before the asynchronous VT transition is
    // reflected by Seat.ActiveSession. Observe that transition, never guess it.
    timeout(Duration::from_secs(2), async {
        while active_session(connection).await? != id {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    })
    .await
    .map_err(|_| unavailable("graphical foreground transition is incomplete"))?
}

pub(crate) async fn activate(
    connection: &Connection,
    root: &Path,
    role: SessionRole,
    target: &GraphicalSession,
) -> Result<(), ResourceControlError> {
    if read_boot_id(root)? != target.boot_id {
        return Err(rejected("graphical activation belongs to another boot"));
    }
    let sessions = local_graphical_sessions(connection, root, role).await?;
    let observed = observe_sessions(&sessions, target.boot_id.clone());
    if observed.state != GraphicalSessionState::Running
        || observed.session.as_ref() != Some(target)
        || observed.locked_hint
    {
        return Err(rejected(
            "graphical activation target is stale, ambiguous or locked",
        ));
    }
    if !desktop_ready(connection, root, role, target).await {
        return Err(unavailable("graphical desktop is not ready"));
    }
    // The user-bus probe awaited external work. Recheck the role immediately
    // before invoking logind, including a lock that appeared during the probe.
    let sessions = local_graphical_sessions(connection, root, role).await?;
    let observed = observe_sessions(&sessions, target.boot_id.clone());
    if observed.state != GraphicalSessionState::Running
        || observed.session.as_ref() != Some(target)
        || observed.locked_hint
    {
        return Err(rejected(
            "graphical activation target changed during observation",
        ));
    }
    let path = &sessions[0].path;
    fresh_proxy(
        connection,
        LOGIN1_SERVICE,
        path.as_str(),
        LOGIN1_SESSION_INTERFACE,
    )
    .await
    .map_err(logind_error)?
    .call::<_, _, ()>("Activate", &())
    .await
    .map_err(logind_error)?;
    wait_for_foreground(connection, &target.logind_session_id).await?;
    let observed = observe(connection, root).await?;
    let (actual, foreground) = match role {
        SessionRole::Waiting => (&observed.waiting, SessionForeground::Waiting),
        SessionRole::Contest => (&observed.contest, SessionForeground::Contest),
    };
    if actual.session.as_ref() != Some(target)
        || !actual.desktop_ready
        || actual.locked_hint
        || observed.foreground != foreground
    {
        return Err(unavailable("graphical activation did not verify"));
    }
    Ok(())
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

pub(crate) async fn terminate(
    connection: &Connection,
    root: &Path,
    target: &GraphicalSession,
) -> Result<(), ResourceControlError> {
    if !valid_boot_id(&target.boot_id) {
        return Err(rejected("graphical session target is invalid"));
    }
    if read_boot_id(root)? != target.boot_id {
        return Ok(());
    }
    let sessions = local_graphical_sessions(connection, root, SessionRole::Contest).await?;
    let Some(path) = exact_termination_path(&sessions, &target.logind_session_id)? else {
        return Ok(());
    };
    if sessions.iter().any(|session| {
        session.id == target.logind_session_id && (!session.is_x11 || session.seat != MANAGED_SEAT)
    }) {
        return Err(rejected("captured contest role no longer matches"));
    }
    fresh_proxy(
        connection,
        LOGIN1_SERVICE,
        path.as_str(),
        LOGIN1_SESSION_INTERFACE,
    )
    .await
    .map_err(logind_error)?
    .call::<_, _, ()>("Terminate", &())
    .await
    .map_err(logind_error)?;
    let remaining = local_graphical_sessions(connection, root, SessionRole::Contest).await?;
    if exact_termination_path(&remaining, &target.logind_session_id)?.is_some() {
        return Err(unavailable("logind session termination is incomplete"));
    }
    Ok(())
}

/// Drain only the captured contest generation. Unexpected logins are a fault,
/// including remote/TTY sessions which ordinary desktop observation ignores.
pub(crate) async fn drain_contest(
    connection: &Connection,
    root: &Path,
    captured: Option<&GraphicalSession>,
) -> Result<(), ResourceControlError> {
    drain(connection, root, SessionRole::Contest, captured).await
}

pub(crate) async fn drain_waiting(
    connection: &Connection,
    root: &Path,
    captured: Option<&GraphicalSession>,
) -> Result<(), ResourceControlError> {
    drain(connection, root, SessionRole::Waiting, captured).await
}

/// GDM may auto-login waiting after the captured session has fully disappeared.
/// Reuse that healthy replacement; never drain a new session as the old one.
pub(crate) async fn waiting_replacement_ready(
    connection: &Connection,
    root: &Path,
    captured: Option<&GraphicalSession>,
) -> Result<bool, ResourceControlError> {
    clear_failed_waiting_scope(connection, root, captured).await?;
    let sessions = local_graphical_sessions(connection, root, SessionRole::Waiting).await?;
    let observed = observe_sessions(&sessions, read_boot_id(root)?);
    let Some(replacement) = observed.session.as_ref().filter(|session| {
        observed.state == GraphicalSessionState::Running
            && !observed.locked_hint
            && Some(*session) != captured
    }) else {
        return Ok(false);
    };
    // This additionally rejects other waiting UID sessions, including TTY/SSH
    // which the graphical observation intentionally does not enumerate.
    require_captured_role(connection, root, SessionRole::Waiting, Some(replacement)).await?;
    Ok(desktop_ready(connection, root, SessionRole::Waiting, replacement).await)
}

/// A timed-out GDM worker can leave an empty failed scope keeping the captured
/// login in `closing`. Reset only that dead scope, even if GDM has already
/// created another waiting login. Ordinary observation never performs cleanup.
async fn clear_failed_waiting_scope(
    connection: &Connection,
    root: &Path,
    captured: Option<&GraphicalSession>,
) -> Result<(), ResourceControlError> {
    let Some(captured) = captured.filter(|s| !s.logind_session_id.is_empty()) else {
        return Ok(());
    };
    if read_boot_id(root)? != captured.boot_id {
        return Ok(());
    }
    let sessions = local_graphical_sessions(connection, root, SessionRole::Waiting).await?;
    let Some(path) = exact_termination_path(&sessions, &captured.logind_session_id)? else {
        return Ok(());
    };
    if !sessions.iter().any(|s| {
        s.id == captured.logind_session_id
            && s.is_x11
            && s.seat == MANAGED_SEAT
            && s.state == GraphicalSessionState::Terminating
    }) {
        return Ok(());
    }
    let session = fresh_proxy(
        connection,
        LOGIN1_SERVICE,
        path.as_str(),
        LOGIN1_SESSION_INTERFACE,
    )
    .await
    .map_err(logind_error)?;
    let scope: String = session.get_property("Scope").await.map_err(logind_error)?;
    if scope != format!("session-{}.scope", captured.logind_session_id) {
        return Err(rejected("captured waiting scope does not match"));
    }
    let leader: u32 = session.get_property("Leader").await.map_err(logind_error)?;
    if leader <= 1 {
        return Err(rejected("captured waiting leader is invalid"));
    }
    match fs::symlink_metadata(root.join("proc").join(leader.to_string())) {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(unavailable("captured waiting leader cannot be inspected")),
    }
    let systemd = fresh_proxy(
        connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await
    .map_err(logind_error)?;
    let path: OwnedObjectPath = match systemd.call("GetUnit", &(&scope,)).await {
        Ok(path) => path,
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.freedesktop.systemd1.NoSuchUnit" =>
        {
            return Ok(());
        }
        Err(error) => return Err(logind_error(error)),
    };
    let unit = fresh_proxy(
        connection,
        "org.freedesktop.systemd1",
        path.as_str(),
        "org.freedesktop.systemd1.Unit",
    )
    .await
    .map_err(logind_error)?;
    let state: String = unit
        .get_property("ActiveState")
        .await
        .map_err(logind_error)?;
    if state != "failed" {
        return Ok(());
    }
    let scope = fresh_proxy(
        connection,
        "org.freedesktop.systemd1",
        path.as_str(),
        "org.freedesktop.systemd1.Scope",
    )
    .await
    .map_err(logind_error)?;
    let cgroup: String = scope
        .get_property("ControlGroup")
        .await
        .map_err(logind_error)?;
    if cgroup.is_empty() {
        unit.call::<_, _, ()>("ResetFailed", &())
            .await
            .map_err(logind_error)?;
    }
    Ok(())
}

async fn drain(
    connection: &Connection,
    root: &Path,
    role: SessionRole,
    captured: Option<&GraphicalSession>,
) -> Result<(), ResourceControlError> {
    use rustix::process::Signal;
    use tokio::time::{Instant, sleep};

    let uid = account(root, account_name(role))?.uid;
    require_captured_role(connection, root, role, captured).await?;
    crate::login::cancel(connection, root, role).await?;
    if let Some(path) = require_captured_role(connection, root, role, captured).await? {
        let result = fresh_proxy(
            connection,
            LOGIN1_SERVICE,
            path.as_str(),
            LOGIN1_SESSION_INTERFACE,
        )
        .await
        .map_err(logind_error)?
        .call::<_, _, ()>("Terminate", &())
        .await;
        if let Err(error) = result {
            require_captured_role(connection, root, role, captured).await?;
            if !matches!(&error, zbus::Error::MethodError(name, _, _) if name.as_str() == "org.freedesktop.login1.NoSuchSession")
            {
                return Err(logind_error(error));
            }
        }
    }
    let systemd = fresh_proxy(
        connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await
    .map_err(|_| unavailable("managed user manager is unavailable"))?;
    let unit = format!("user@{uid}.service");
    let path: OwnedObjectPath = systemd
        .call("LoadUnit", &(&unit,))
        .await
        .map_err(|_| unavailable("managed user manager is unavailable"))?;
    let _: OwnedObjectPath = systemd
        .call("StopUnit", &(&unit, "replace"))
        .await
        .map_err(|_| unavailable("managed user manager cannot be stopped"))?;
    let user = fresh_proxy(
        connection,
        "org.freedesktop.systemd1",
        path.as_str(),
        "org.freedesktop.systemd1.Unit",
    )
    .await
    .map_err(|_| unavailable("managed user manager is unavailable"))?;
    let started = Instant::now();
    loop {
        let remaining = require_captured_role(connection, root, role, captured).await?;
        let processes = crate::processes::owned_by(root, uid)?;
        let state: String = user
            .get_property("ActiveState")
            .await
            .map_err(|_| unavailable("managed user manager state is unavailable"))?;
        if remaining.is_none()
            && processes.is_empty()
            && matches!(state.as_str(), "inactive" | "failed")
            && (role == SessionRole::Waiting || crate::admission::require_no_workers(root).is_ok())
        {
            return Ok(());
        }
        let signal = if started.elapsed() < Duration::from_secs(1) {
            Signal::TERM
        } else {
            Signal::KILL
        };
        if role == SessionRole::Contest {
            crate::admission::stop_workers(root, signal)?;
        }
        for identity in processes {
            crate::processes::signal(identity, signal)?;
        }
        if signal == Signal::KILL
            && let Some(path) = remaining
        {
            // UID drainage does not reach the root GDM worker. Its exact
            // captured session scope is also ours to terminate, not GDM itself.
            fresh_proxy(
                connection,
                LOGIN1_SERVICE,
                path.as_str(),
                LOGIN1_SESSION_INTERFACE,
            )
            .await
            .map_err(logind_error)?
            .call::<_, _, ()>("Kill", &("all", 9_i32))
            .await
            .map_err(logind_error)?;
        }
        if started.elapsed() >= Duration::from_secs(5) {
            return Err(unavailable("managed user drainage is incomplete"));
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn require_captured_role(
    connection: &Connection,
    root: &Path,
    role: SessionRole,
    captured: Option<&GraphicalSession>,
) -> Result<Option<OwnedObjectPath>, ResourceControlError> {
    let name = account_name(role);
    let uid = account(root, name)?.uid;
    let boot = read_boot_id(root)?;
    let mut matching = listed_sessions(connection)
        .await?
        .into_iter()
        .filter(|(_, actual_uid, actual_name, _, _)| *actual_uid == uid || actual_name == name);
    let Some((id, actual_uid, actual_name, seat, path)) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some()
        || actual_uid != uid
        || actual_name != name
        || seat != MANAGED_SEAT
        || captured
            .is_none_or(|expected| expected.boot_id != boot || expected.logind_session_id != id)
    {
        return Err(rejected("managed drainage found an uncaptured session"));
    }
    let session = fresh_proxy(
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
    if remote || class != "user" || kind != "x11" {
        return Err(rejected("managed drainage found an unsupported session"));
    }
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use zbus::zvariant::OwnedObjectPath;

    use natsume_local_control_api::{GraphicalSession, GraphicalSessionState, SessionRole};

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
            state: GraphicalSessionState::Running,
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

    #[test]
    fn fixed_accounts_reject_missing_malformed_root_and_uid_aliases()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = TempDir::new()?;
        fs::create_dir(fixture.path().join("etc"))?;
        for contents in [
            "",
            "waiting:x:1002:1002",
            "waiting:x:0:0::/home/waiting:/bin/bash\n",
            "waiting:x:1002:1002::/home/waiting:/bin/bash\nalias:x:1002:1002::/:/bin/bash\n",
            "waiting:x:1002:1002::/home/waiting:/bin/bash\nwaiting:x:1003:1003::/:/bin/bash\n",
        ] {
            fs::write(fixture.path().join("etc/passwd"), contents)?;
            assert!(super::account(fixture.path(), "waiting").is_err());
        }
        fs::write(
            fixture.path().join("etc/passwd"),
            "waiting:x:1002:1002::/home/waiting:/bin/bash\n",
        )?;
        assert_eq!(super::account(fixture.path(), "waiting")?.uid, 1002);
        Ok(())
    }

    #[test]
    fn contest_role_resolves_teams_without_adopting_a_legacy_named_account()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = TempDir::new()?;
        fs::create_dir(fixture.path().join("etc"))?;
        let passwd = fixture.path().join("etc/passwd");
        let legacy = "contest:x:1001:1001::/home/contest:/bin/bash\n";
        fs::write(&passwd, legacy)?;
        assert!(super::account(fixture.path(), super::account_name(SessionRole::Contest)).is_err());
        fs::write(
            &passwd,
            format!("{legacy}teams:x:1101:1101::/home/teams:/bin/bash\n"),
        )?;
        assert_eq!(
            super::account(fixture.path(), super::account_name(SessionRole::Contest))?.uid,
            1101,
        );
        assert_eq!(super::role_name(SessionRole::Contest), "contest");
        Ok(())
    }

    #[test]
    fn incomplete_lifecycles_keep_identity_without_claiming_readiness() {
        for state in [
            GraphicalSessionState::Starting,
            GraphicalSessionState::Terminating,
        ] {
            let mut session = candidate("c2", "/org/freedesktop/login1/session/c2", "seat0");
            session.state = state;
            let observed = observe_sessions(
                &[session],
                "550e8400-e29b-41d4-a716-446655440000".to_owned(),
            );
            assert_eq!(observed.state, state);
            assert!(observed.session.is_some());
            assert!(!observed.desktop_ready);
        }
    }

    struct TestSeat(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    #[zbus::interface(name = "org.freedesktop.login1.Seat")]
    impl TestSeat {
        #[zbus(property)]
        fn active_session(&self) -> (String, OwnedObjectPath) {
            let reads = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let id = if reads == 0 { "old" } else { "c2" };
            (
                id.to_owned(),
                OwnedObjectPath::try_from("/session")
                    .unwrap_or_else(|error| panic!("fixture path: {error}")),
            )
        }
    }

    struct TestLogin;

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl TestLogin {
        #[allow(clippy::unused_self)]
        fn get_seat(&self, seat: &str) -> zbus::fdo::Result<OwnedObjectPath> {
            assert_eq!(seat, "seat0");
            OwnedObjectPath::try_from("/seat")
                .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
        }
    }

    #[tokio::test]
    async fn activation_waits_for_the_observed_foreground_transition()
    -> Result<(), Box<dyn std::error::Error>> {
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (server, client) = tokio::net::UnixStream::pair()?;
        let server = zbus::connection::Builder::unix_stream(server)
            .server(zbus::Guid::generate())?
            .p2p()
            .serve_at("/org/freedesktop/login1", TestLogin)?
            .serve_at("/seat", TestSeat(reads.clone()))?;
        let client = zbus::connection::Builder::unix_stream(client).p2p();
        let (_server, client) = tokio::try_join!(server.build(), client.build())?;
        super::wait_for_foreground(&client, "c2").await?;
        assert!(reads.load(std::sync::atomic::Ordering::SeqCst) >= 2);
        assert!(
            super::wait_for_foreground(&client, "never-active")
                .await
                .is_err()
        );
        Ok(())
    }

    struct ScopeCase {
        session_state: &'static str,
        seat: &'static str,
        kind: &'static str,
        uid: u32,
        scope: &'static str,
        unit_state: &'static str,
        cgroup: Option<&'static str>,
    }

    impl Default for ScopeCase {
        fn default() -> Self {
            Self {
                session_state: "closing",
                seat: "seat0",
                kind: "x11",
                uid: 1002,
                scope: "session-c1.scope",
                unit_state: "failed",
                cgroup: Some(""),
            }
        }
    }

    struct ScopeLogin(std::sync::Arc<ScopeCase>);

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl ScopeLogin {
        fn list_sessions(&self) -> Vec<super::ListedSession> {
            [
                ("c1", self.0.uid, self.0.seat, "/old"),
                ("c2", 1002, "seat0", "/new"),
            ]
            .into_iter()
            .map(|(id, uid, seat, path)| {
                (
                    id.to_owned(),
                    uid,
                    "waiting".to_owned(),
                    seat.to_owned(),
                    OwnedObjectPath::try_from(path).unwrap_or_else(|e| panic!("fixture path: {e}")),
                )
            })
            .collect()
        }
    }

    struct ScopeSession {
        case: std::sync::Arc<ScopeCase>,
        old: bool,
    }

    #[zbus::interface(name = "org.freedesktop.login1.Session")]
    #[allow(clippy::unused_self)]
    impl ScopeSession {
        #[zbus(property)]
        fn remote(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn class(&self) -> &'static str {
            "user"
        }
        #[zbus(property, name = "Type")]
        fn kind(&self) -> &str {
            self.case.kind
        }
        #[zbus(property)]
        fn locked_hint(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn state(&self) -> &str {
            if self.old {
                self.case.session_state
            } else {
                "active"
            }
        }
        #[zbus(property)]
        fn scope(&self) -> &str {
            if self.old {
                self.case.scope
            } else {
                "session-c2.scope"
            }
        }
        #[zbus(property)]
        fn leader(&self) -> u32 {
            610
        }
    }

    struct ScopeManager;

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    #[allow(clippy::unused_self)]
    impl ScopeManager {
        fn get_unit(&self, name: &str) -> OwnedObjectPath {
            assert_eq!(
                name, "session-c1.scope",
                "must never address the replacement scope"
            );
            OwnedObjectPath::try_from("/old_scope").unwrap_or_else(|e| panic!("fixture path: {e}"))
        }
    }

    struct ScopeUnit {
        case: std::sync::Arc<ScopeCase>,
        resets: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Unit")]
    impl ScopeUnit {
        #[zbus(property)]
        fn active_state(&self) -> &str {
            self.case.unit_state
        }
        fn reset_failed(&self) {
            self.resets
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    struct ScopeControl(std::sync::Arc<ScopeCase>);

    #[zbus::interface(name = "org.freedesktop.systemd1.Scope")]
    impl ScopeControl {
        #[zbus(property)]
        fn control_group(&self) -> zbus::fdo::Result<&str> {
            self.0
                .cgroup
                .ok_or_else(|| zbus::fdo::Error::Failed("fixture observation failure".to_owned()))
        }
    }

    #[tokio::test]
    async fn only_the_captured_dead_empty_failed_waiting_scope_is_reset()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let boot = "550e8400-e29b-41d4-a716-446655440000";
        for name in [
            "dead",
            "uncaptured",
            "old-boot",
            "missing",
            "replacement",
            "live-leader",
            "active-login",
            "wrong-seat",
            "wayland",
            "wrong-uid",
            "wrong-scope",
            "active-scope",
            "occupied-scope",
            "unreadable-cgroup",
        ] {
            let mut case = ScopeCase::default();
            let mut captured = Some(("c1", boot));
            match name {
                "uncaptured" => captured = None,
                "old-boot" => captured = Some(("c1", "550e8400-e29b-41d4-a716-446655440001")),
                "missing" => captured = Some(("missing", boot)),
                "replacement" => captured = Some(("c2", boot)),
                "active-login" => case.session_state = "active",
                "wrong-seat" => case.seat = "seat1",
                "wayland" => case.kind = "wayland",
                "wrong-uid" => case.uid = 1001,
                "wrong-scope" => case.scope = "session-c2.scope",
                "active-scope" => case.unit_state = "active",
                "occupied-scope" => case.cgroup = Some("/user.slice/occupied"),
                "unreadable-cgroup" => case.cgroup = None,
                "dead" | "live-leader" => {}
                _ => unreachable!(),
            }
            let root = TempDir::new()?;
            fs::create_dir_all(root.path().join("proc/sys/kernel/random"))?;
            fs::create_dir_all(root.path().join("etc"))?;
            fs::write(root.path().join(super::BOOT_ID_PATH), boot)?;
            fs::write(
                root.path().join("etc/passwd"),
                "waiting:x:1002:1002::/home/waiting:/bin/bash\n",
            )?;
            if name == "live-leader" {
                fs::create_dir(root.path().join("proc/610"))?;
            }
            let case = Arc::new(case);
            let resets = Arc::new(AtomicUsize::new(0));
            let (server, client) = tokio::net::UnixStream::pair()?;
            let server = zbus::connection::Builder::unix_stream(server)
                .server(zbus::Guid::generate())?
                .p2p()
                .serve_at("/org/freedesktop/login1", ScopeLogin(case.clone()))?
                .serve_at(
                    "/old",
                    ScopeSession {
                        case: case.clone(),
                        old: true,
                    },
                )?
                .serve_at(
                    "/new",
                    ScopeSession {
                        case: case.clone(),
                        old: false,
                    },
                )?
                .serve_at("/org/freedesktop/systemd1", ScopeManager)?
                .serve_at(
                    "/old_scope",
                    ScopeUnit {
                        case: case.clone(),
                        resets: resets.clone(),
                    },
                )?
                .serve_at("/old_scope", ScopeControl(case))?;
            let client = zbus::connection::Builder::unix_stream(client).p2p();
            let (_server, client) = tokio::try_join!(server.build(), client.build())?;
            let captured = captured.map(|(id, boot)| GraphicalSession {
                logind_session_id: id.to_owned(),
                boot_id: boot.to_owned(),
            });
            let result =
                super::clear_failed_waiting_scope(&client, root.path(), captured.as_ref()).await;
            assert_eq!(
                result.is_err(),
                matches!(name, "wrong-scope" | "unreadable-cgroup"),
                "{name}"
            );
            assert_eq!(
                resets.load(Ordering::SeqCst),
                usize::from(name == "dead"),
                "{name}"
            );
        }
        Ok(())
    }
}
