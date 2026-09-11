use std::{path::Path, process::Stdio};

use futures_util::StreamExt as _;
use natsume_local_control_api::{
    GraphicalSessionObservation, GraphicalSessionState, ManagedSessionsObservation,
    ResourceControlError, SessionRole,
};
use tokio::{
    process::Command,
    time::{Duration, Instant, sleep, timeout},
};
use zbus::{Connection, MessageStream, message::Type, zvariant::OwnedObjectPath};

use crate::{
    admission,
    session::{self, account_name, fresh_proxy, rejected, role_name, unavailable},
};

const HELPER: &str = "/usr/lib/natsume/natsume-privileged-helper";
const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const GDM: &str = "org.gnome.DisplayManager";
const PEER_PATH: &str = "/org/gnome/DisplayManager/Session";
const GREETER: &str = "org.gnome.DisplayManager.Greeter";
const VERIFIER: &str = "org.gnome.DisplayManager.UserVerifier";
const PREPARE_TIMEOUT: Duration = Duration::from_secs(40);

fn unit(role: SessionRole) -> &'static str {
    match role {
        SessionRole::Waiting => "natsume-session-prepare@waiting.service",
        SessionRole::Contest => "natsume-session-prepare@contest.service",
    }
}

fn role_observation(
    observation: ManagedSessionsObservation,
    role: SessionRole,
) -> GraphicalSessionObservation {
    match role {
        SessionRole::Waiting => observation.waiting,
        SessionRole::Contest => observation.contest,
    }
}

async fn unit_state(
    connection: &Connection,
    role: SessionRole,
) -> Result<String, ResourceControlError> {
    let manager = fresh_proxy(
        connection,
        SYSTEMD,
        SYSTEMD_PATH,
        "org.freedesktop.systemd1.Manager",
    )
    .await
    .map_err(|_| unavailable("session preparation service is unavailable"))?;
    let path: OwnedObjectPath = manager
        .call("LoadUnit", &(unit(role),))
        .await
        .map_err(|_| unavailable("session preparation service is unavailable"))?;
    fresh_proxy(
        connection,
        SYSTEMD,
        path.as_str(),
        "org.freedesktop.systemd1.Unit",
    )
    .await
    .map_err(|_| unavailable("session preparation service is unavailable"))?
    .get_property("ActiveState")
    .await
    .map_err(|_| unavailable("session preparation state is unavailable"))
}

fn unit_busy(state: &str) -> bool {
    matches!(state, "activating" | "active" | "deactivating")
}

async fn unit_job(
    connection: &Connection,
    role: SessionRole,
    start: bool,
) -> Result<(), ResourceControlError> {
    let manager = fresh_proxy(
        connection,
        SYSTEMD,
        SYSTEMD_PATH,
        "org.freedesktop.systemd1.Manager",
    )
    .await
    .map_err(|_| unavailable("session preparation service is unavailable"))?;
    let _: OwnedObjectPath = manager
        .call(
            if start { "StartUnit" } else { "StopUnit" },
            &(unit(role), "replace"),
        )
        .await
        .map_err(|_| unavailable("session preparation transition is unavailable"))?;
    Ok(())
}

pub(crate) async fn observe(
    connection: &Connection,
    root: &Path,
) -> Result<ManagedSessionsObservation, ResourceControlError> {
    let mut observed = session::observe(connection, root).await?;
    for (role, current) in [
        (SessionRole::Waiting, &mut observed.waiting),
        (SessionRole::Contest, &mut observed.contest),
    ] {
        if current.state == GraphicalSessionState::None {
            current.state = match unit_state(connection, role).await?.as_str() {
                "activating" | "active" => GraphicalSessionState::Starting,
                "deactivating" => GraphicalSessionState::Terminating,
                "failed" => GraphicalSessionState::Error,
                "inactive" => GraphicalSessionState::None,
                _ => return Err(unavailable("session preparation state is indeterminate")),
            };
        }
    }
    Ok(observed)
}

pub(crate) async fn start(
    connection: &Connection,
    root: &Path,
    role: SessionRole,
) -> Result<GraphicalSessionObservation, ResourceControlError> {
    let _mutation = admission::mutation(root)?;
    start_owned(connection, root, role).await
}

pub(crate) async fn start_owned(
    connection: &Connection,
    root: &Path,
    role: SessionRole,
) -> Result<GraphicalSessionObservation, ResourceControlError> {
    if role == SessionRole::Waiting {
        crate::waiting::require_login_allowed(root)?;
    }
    let current = role_observation(session::observe(connection, root).await?, role);
    match current.state {
        GraphicalSessionState::Running
        | GraphicalSessionState::Starting
        | GraphicalSessionState::Terminating => return Ok(current),
        GraphicalSessionState::Ambiguous | GraphicalSessionState::Error => {
            return Err(rejected("session preparation requires an unambiguous role"));
        }
        GraphicalSessionState::None => {}
    }
    if role == SessionRole::Contest {
        admission::require_open(root)?;
        crate::home::require_template(root)?;
    }
    for candidate in [SessionRole::Waiting, SessionRole::Contest] {
        if unit_busy(&unit_state(connection, candidate).await?) {
            return if candidate == role {
                Ok(GraphicalSessionObservation {
                    state: GraphicalSessionState::Starting,
                    ..current
                })
            } else {
                Err(unavailable("another session preparation is in progress"))
            };
        }
    }
    if role == SessionRole::Contest {
        admission::require_no_workers(root)?;
    }
    unit_job(connection, role, true).await?;
    Ok(GraphicalSessionObservation {
        state: GraphicalSessionState::Starting,
        ..current
    })
}

pub(crate) async fn cancel(
    connection: &Connection,
    root: &Path,
    role: SessionRole,
) -> Result<(), ResourceControlError> {
    if role == SessionRole::Contest {
        admission::close(root)?;
    }
    unit_job(connection, role, false).await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while unit_busy(&unit_state(connection, role).await?) {
        if Instant::now() >= deadline {
            return Err(unavailable("managed login cancellation is incomplete"));
        }
        sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

pub(crate) async fn close_contest(
    connection: &Connection,
    root: &Path,
) -> Result<(), ResourceControlError> {
    // Withdrawal precedes cancellation. Never wait for PAM/GDM while holding the
    // admission lock needed by those processes to finish their callbacks.
    admission::close(root)?;
    unit_job(connection, SessionRole::Contest, false).await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if !unit_busy(&unit_state(connection, SessionRole::Contest).await?) {
            match admission::maintenance(root) {
                Ok(guard) => {
                    drop(guard);
                    return Ok(());
                }
                Err(ResourceControlError::Unavailable(_)) if Instant::now() < deadline => {}
                Err(error) => return Err(error),
            }
        }
        if Instant::now() >= deadline {
            return Err(unavailable("contest login cancellation did not drain"));
        }
        sleep(Duration::from_millis(50)).await;
    }
}

async fn mutation_when_available(
    root: &Path,
) -> Result<admission::MutationGuard, ResourceControlError> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match admission::mutation(root) {
            Ok(guard) => return Ok(guard),
            Err(ResourceControlError::Unavailable(_)) if Instant::now() < deadline => {
                sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Fixed root systemd entry. Owns only preparation; GDM owns the resulting desktop.
///
/// # Errors
/// Rejects conflicting roles, closed admission and failed GDM preparation.
pub async fn run_prepare(role: SessionRole) -> Result<(), ResourceControlError> {
    if !rustix::process::geteuid().is_root() {
        return Err(rejected("session preparation requires root"));
    }
    timeout(PREPARE_TIMEOUT, prepare_inner(role))
        .await
        .map_err(|_| unavailable("session preparation timed out; resample before retry"))?
}

async fn prepare_inner(role: SessionRole) -> Result<(), ResourceControlError> {
    let root = Path::new("/");
    let _mutation = mutation_when_available(root).await?;
    if role == SessionRole::Waiting {
        crate::waiting::require_login_allowed(root)?;
    }
    let connection = Connection::system()
        .await
        .map_err(|_| unavailable("system bus is unavailable"))?;
    let current = role_observation(session::observe(&connection, root).await?, role);
    if current.state != GraphicalSessionState::None {
        return if current.state == GraphicalSessionState::Running && current.desktop_ready {
            Ok(())
        } else {
            Err(rejected(
                "session preparation cannot replace an existing role",
            ))
        };
    }
    if role == SessionRole::Contest {
        admission::require_open(root)?;
        crate::home::require_template(root)?;
        admission::require_no_workers(root)?;
    }
    if session::greeter(&connection, root).await?.is_none() {
        let factory = fresh_proxy(
            &connection,
            GDM,
            "/org/gnome/DisplayManager/LocalDisplayFactory",
            "org.gnome.DisplayManager.LocalDisplayFactory",
        )
        .await
        .map_err(|_| unavailable("GDM display factory is unavailable"))?;
        let _: OwnedObjectPath = factory
            .call("CreateTransientDisplay", &())
            .await
            .map_err(|_| unavailable("GDM greeter creation failed"))?;
    }
    let (id, path) = loop {
        if let Some(greeter) = session::greeter(&connection, root).await? {
            break greeter;
        }
        sleep(Duration::from_millis(100)).await;
    };
    session::activate_greeter(&connection, &id, &path).await?;
    if role == SessionRole::Contest {
        admission::require_open(root)?;
        crate::home::require_template(root)?;
    }
    let gdm = session::account(root, "gdm")?;
    let mut child = Command::new("/usr/bin/setpriv")
        .args([
            "--reuid",
            &gdm.uid.to_string(),
            "--regid",
            &gdm.gid.to_string(),
            "--clear-groups",
            "--no-new-privs",
            HELPER,
            "gdm-login",
            role_name(role),
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| unavailable("fixed GDM client could not start"))?;
    loop {
        let current = role_observation(session::observe(&connection, root).await?, role);
        if current.state == GraphicalSessionState::Running
            && current.desktop_ready
            && !current.locked_hint
        {
            // Close the API connection only after the desktop has independent
            // GNOME readiness. The GDM worker/session is outside this unit.
            let _killed = child.kill().await;
            let _reaped = child.wait().await;
            return Ok(());
        }
        if matches!(
            current.state,
            GraphicalSessionState::Ambiguous | GraphicalSessionState::Error
        ) {
            return Err(rejected("GDM prepared an invalid graphical role"));
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|_| unavailable("GDM client state is unavailable"))?
            && !status.success()
        {
            return Err(unavailable("GDM authentication failed"));
        }
        sleep(Duration::from_millis(100)).await;
    }
}

/// Same official peer API used by libgdm, under the fixed GDM Unix identity.
///
/// # Errors
/// Rejects a wrong Unix identity or unsuccessful fixed GDM authentication.
pub async fn run_gdm_client(role: SessionRole) -> Result<(), ResourceControlError> {
    let account = session::account(Path::new("/"), "gdm")?;
    if rustix::process::geteuid().as_raw() != account.uid
        || rustix::process::getuid().as_raw() != account.uid
    {
        return Err(rejected("GDM client requires the fixed gdm identity"));
    }
    timeout(PREPARE_TIMEOUT, async {
        let bus = Connection::system()
            .await
            .map_err(|_| unavailable("system bus is unavailable"))?;
        // logind can report the greeter active before its Xorg handles VT release.
        // Starting a user session then can strand GDM's worker in VT_WAITACTIVE.
        // Wait for that greeter's GNOME startup before beginning authentication.
        while !greeter_desktop_ready(&bus).await {
            sleep(Duration::from_millis(100)).await;
        }
        authenticate(role, &bus).await
    })
    .await
    .map_err(|_| unavailable("GDM authentication timed out"))?
}

async fn authenticate(role: SessionRole, bus: &Connection) -> Result<(), ResourceControlError> {
    let manager = fresh_proxy(
        bus,
        GDM,
        "/org/gnome/DisplayManager/Manager",
        "org.gnome.DisplayManager.Manager",
    )
    .await
    .map_err(|_| unavailable("GDM manager is unavailable"))?;
    let peer = loop {
        let result: zbus::Result<String> = manager.call("OpenSession", &()).await;
        if let Ok(address) = result {
            break zbus::connection::Builder::address(address.as_str())
                .map_err(|_| rejected("GDM peer address is invalid"))?
                .p2p()
                .method_timeout(Duration::from_secs(5))
                .build()
                .await
                .map_err(|_| unavailable("GDM peer connection failed"))?;
        }
        sleep(Duration::from_millis(100)).await;
    };
    let mut messages = MessageStream::from(&peer);
    let service = match role {
        SessionRole::Waiting => "gdm-waiting",
        SessionRole::Contest => "gdm-contest",
    };
    let desktop = match role {
        SessionRole::Waiting => "gnome-kiosk-script-xorg",
        SessionRole::Contest => "ubuntu-xorg",
    };
    peer.call_method(
        None::<&str>,
        PEER_PATH,
        Some(GREETER),
        "SelectSession",
        &(desktop,),
    )
    .await
    .map_err(|_| unavailable("GDM X11 selection failed"))?;
    peer.call_method(
        None::<&str>,
        PEER_PATH,
        Some(VERIFIER),
        "BeginVerificationForUser",
        &(service, account_name(role)),
    )
    .await
    .map_err(|_| unavailable("GDM verification could not begin"))?;
    let mut opened = false;
    while let Some(message) = messages.next().await {
        let message = message.map_err(|_| unavailable("GDM authentication channel failed"))?;
        if message.message_type() != Type::Signal {
            continue;
        }
        let header = message.header();
        let member = header.member().map(zbus::names::MemberName::as_str);
        if header.interface().map(zbus::names::InterfaceName::as_str) == Some(GREETER)
            && member == Some("SessionOpened")
        {
            let (actual_service, _id): (String, String) = message
                .body()
                .deserialize()
                .map_err(|_| rejected("GDM session signal is invalid"))?;
            if actual_service != service || opened {
                return Err(rejected("GDM session signal does not match the request"));
            }
            peer.call_method(
                None::<&str>,
                PEER_PATH,
                Some(GREETER),
                "StartSessionWhenReady",
                &(service, true),
            )
            .await
            .map_err(|_| unavailable("GDM session start failed"))?;
            opened = true;
        }
        if header.interface().map(zbus::names::InterfaceName::as_str) == Some(VERIFIER)
            && matches!(
                member,
                Some("VerificationFailed" | "ServiceUnavailable" | "InfoQuery" | "SecretInfoQuery")
            )
        {
            let _cancelled = peer
                .call_method(None::<&str>, PEER_PATH, Some(VERIFIER), "Cancel", &())
                .await;
            return Err(rejected("fixed GDM authentication was rejected"));
        }
    }
    if opened {
        Ok(())
    } else {
        Err(unavailable("GDM disconnected before opening the session"))
    }
}

async fn greeter_desktop_ready(connection: &Connection) -> bool {
    let root = Path::new("/");
    let uid = rustix::process::geteuid().as_raw();
    let operation = async {
        let (_, path) = session::greeter(connection, root).await.ok()??;
        let greeter = fresh_proxy(
            connection,
            "org.freedesktop.login1",
            path.as_str(),
            "org.freedesktop.login1.Session",
        )
        .await
        .ok()?;
        if greeter.get_property::<String>("Type").await.ok()? != "x11"
            || !greeter.get_property::<bool>("Active").await.ok()?
        {
            return None;
        }
        let logind = fresh_proxy(
            connection,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )
        .await
        .ok()?;
        for identity in crate::processes::owned_by(root, uid).ok()? {
            let Ok(process) = procfs::process::Process::new(identity.pid) else {
                continue;
            };
            if process.exe().ok().as_deref() != Some(Path::new("/usr/bin/gnome-shell")) {
                continue;
            }
            let pid = u32::try_from(identity.pid).ok()?;
            let owner_session: OwnedObjectPath =
                logind.call("GetSessionByPID", &(pid,)).await.ok()?;
            if owner_session != path {
                continue;
            }
            // The greeter uses dbus-run-session, not the fixed user-manager bus.
            // Read only this gdm-owned Shell's address under the same Unix UID.
            let environment = process.environ().ok()?;
            let address = environment
                .get(std::ffi::OsStr::new("DBUS_SESSION_BUS_ADDRESS"))?
                .to_str()?;
            if !address.starts_with("unix:") {
                return None;
            }
            let bus = zbus::connection::Builder::address(address)
                .ok()?
                .method_timeout(Duration::from_secs(1))
                .build()
                .await
                .ok()?;
            let dbus = zbus::fdo::DBusProxy::new(&bus).await.ok()?;
            let shell = dbus
                .get_name_owner("org.gnome.Shell".try_into().ok()?)
                .await
                .ok()?;
            if dbus
                .get_connection_unix_process_id(shell.clone().into())
                .await
                .ok()?
                != pid
                || dbus.get_connection_unix_user(shell.into()).await.ok()? != uid
            {
                return None;
            }
            let manager = dbus
                .get_name_owner("org.gnome.SessionManager".try_into().ok()?)
                .await
                .ok()?;
            if dbus.get_connection_unix_user(manager.into()).await.ok()? != uid {
                return None;
            }
            let gnome = fresh_proxy(
                &bus,
                "org.gnome.SessionManager",
                "/org/gnome/SessionManager",
                "org.gnome.SessionManager",
            )
            .await
            .ok()?;
            let running: bool = gnome.call("IsSessionRunning", &()).await.ok()?;
            return (running
                && crate::processes::start_time(root, identity.pid).ok()? == Some(identity.start)
                && greeter.get_property::<bool>("Active").await.ok()?)
            .then_some(());
        }
        None
    };
    matches!(
        timeout(Duration::from_secs(1), operation).await,
        Ok(Some(()))
    )
}
