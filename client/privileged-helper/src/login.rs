use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::io::AsyncReadExt as _;

use futures_util::StreamExt as _;
use natsume_local_control_api::{
    GraphicalSessionObservation, GraphicalSessionState, ManagedSessionsObservation,
    ResourceControlError, SessionRole,
};
use tokio::time::{Duration, Instant, sleep, timeout};
use zbus::{Connection, MessageStream, message::Type, zvariant::OwnedObjectPath};

use crate::{
    admission, gdm_registration,
    session::{self, account_name, fresh_proxy, rejected, unavailable},
};

const HELPER: &str = "/usr/lib/natsume/natsume-privileged-helper";
const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const GDM: &str = "org.gnome.DisplayManager";
const PEER_PATH: &str = "/org/gnome/DisplayManager/Session";
const GREETER: &str = "org.gnome.DisplayManager.Greeter";
const VERIFIER: &str = "org.gnome.DisplayManager.UserVerifier";
const PREPARE_TIMEOUT: Duration = Duration::from_secs(40);

/// The private gdm UID probe's machine output. It does not claim registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GreeterStatus {
    pub(crate) identity: gdm_registration::Greeter,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginContext {
    proof: gdm_registration::Proof,
    greeter: GreeterStatus,
}

mod preparation;

pub(crate) async fn preparing(connection: &Connection) -> Result<bool, ResourceControlError> {
    for role in [SessionRole::Waiting, SessionRole::Contest] {
        if unit_busy(&unit_state(connection, role).await?) {
            return Ok(true);
        }
    }
    Ok(false)
}

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
        let preparation = unit_state(connection, role).await?;
        if unit_busy(&preparation)
            && matches!(
                current.state,
                GraphicalSessionState::None
                    | GraphicalSessionState::Starting
                    | GraphicalSessionState::Running
            )
        {
            current.state = GraphicalSessionState::Starting;
            current.desktop_ready = false;
        } else if current.state == GraphicalSessionState::None {
            current.state = match preparation.as_str() {
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
        GraphicalSessionState::Running if current.desktop_ready => return Ok(current),
        GraphicalSessionState::Running => {
            if preparation::failed_session(root, role, current.session.as_ref())? {
                return Ok(current);
            }
        }
        GraphicalSessionState::Starting | GraphicalSessionState::Terminating => return Ok(current),
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
    if role == SessionRole::Contest && current.session.is_none() {
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
    preparation::run(role).await
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
    let parent =
        rustix::process::getppid().ok_or_else(|| rejected("fixed GDM client has no parent"))?;
    let parent_status = procfs::process::Process::new(parent.as_raw_nonzero().get())
        .and_then(|p| p.status())
        .map_err(|_| rejected("fixed GDM client parent is unavailable"))?;
    if parent_status.ruid != 0 || parent_status.euid != 0 {
        return Err(rejected(
            "fixed GDM client requires a root preparation parent",
        ));
    }
    timeout(PREPARE_TIMEOUT, async {
        let mut input = Vec::new();
        tokio::io::stdin()
            .take(8193)
            .read_to_end(&mut input)
            .await
            .map_err(|_| rejected("captured GDM context cannot be read"))?;
        if input.len() > 8192 {
            return Err(rejected("captured GDM context is too large"));
        }
        let expected: LoginContext = serde_json::from_slice(&input)
            .map_err(|_| rejected("fixed GDM client requires captured greeter context"))?;
        let bus = Connection::system()
            .await
            .map_err(|_| unavailable("system bus is unavailable"))?;
        if expected.proof.greeter != expected.greeter.identity
            || !gdm_registration::context_matches(Path::new("/"), &bus, &expected.proof).await?
            || greeter_status(&bus).await.as_ref() != Some(&expected.greeter)
        {
            return Err(rejected(
                "greeter_replaced: captured GDM identity is no longer current",
            ));
        }
        authenticate(role, &bus, &expected).await
    })
    .await
    .map_err(|_| unavailable("GDM authentication timed out"))?
}

async fn authenticate(
    role: SessionRole,
    bus: &Connection,
    expected: &LoginContext,
) -> Result<(), ResourceControlError> {
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
        SessionRole::Waiting => "gnome-kiosk-script-wayland",
        SessionRole::Contest => "ubuntu-wayland",
    };
    require_current_greeter(bus, expected).await?;
    peer.call_method(
        None::<&str>,
        PEER_PATH,
        Some(GREETER),
        "SelectSession",
        &(desktop,),
    )
    .await
    .map_err(|_| unavailable("GDM Wayland selection failed"))?;
    require_current_greeter(bus, expected).await?;
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
            require_current_greeter(bus, expected).await?;
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

async fn require_current_greeter(
    connection: &Connection,
    expected: &LoginContext,
) -> Result<(), ResourceControlError> {
    if !gdm_registration::context_matches(Path::new("/"), connection, &expected.proof).await?
        || greeter_status(connection).await.as_ref() != Some(&expected.greeter)
    {
        return Err(rejected(
            "captured GDM greeter changed before authentication",
        ));
    }
    Ok(())
}

async fn greeter_candidate(
    connection: &Connection,
    path: &OwnedObjectPath,
    id: &str,
    uid: u32,
    identity: crate::processes::Identity,
) -> Option<GreeterStatus> {
    let process = procfs::process::Process::new(identity.pid).ok()?;
    if process.exe().ok().as_deref() != Some(Path::new("/usr/bin/gnome-shell")) {
        return None;
    }
    let pid = u32::try_from(identity.pid).ok()?;
    let logind = fresh_proxy(
        connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await
    .ok()?;
    let owner_session: OwnedObjectPath = logind.call("GetSessionByPID", &(pid,)).await.ok()?;
    if owner_session != *path {
        return None;
    }
    // This greeter has a private session bus. Read it only from the captured
    // gdm-owned Shell, then verify both the bus owner and the native socket.
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
    if !gnome
        .call::<_, _, bool>("IsSessionRunning", &())
        .await
        .ok()?
    {
        return None;
    }
    crate::display::probe(uid, identity.pid, true).ok()?;
    (crate::processes::start_time(Path::new("/"), identity.pid).ok()? == Some(identity.start)).then(
        || GreeterStatus {
            identity: gdm_registration::Greeter {
                session: id.to_owned(),
                uid,
                pid: identity.pid,
                start: identity.start,
            },
        },
    )
}

async fn greeter_status(connection: &Connection) -> Option<GreeterStatus> {
    let root = Path::new("/");
    let uid = rustix::process::geteuid().as_raw();
    let operation = async {
        let (id, path) = session::greeter(connection, root).await.ok()??;
        let greeter = fresh_proxy(
            connection,
            "org.freedesktop.login1",
            path.as_str(),
            "org.freedesktop.login1.Session",
        )
        .await
        .ok()?;
        if greeter.get_property::<String>("Type").await.ok()? != "wayland"
            || !greeter.get_property::<bool>("Active").await.ok()?
        {
            return None;
        }
        let mut found = None;
        for identity in crate::processes::owned_by(root, uid).ok()? {
            if let Some(candidate) = greeter_candidate(connection, &path, &id, uid, identity).await
            {
                if found.is_some() {
                    return None;
                }
                found = Some(candidate);
            }
        }
        if !greeter.get_property::<bool>("Active").await.ok()? {
            return None;
        }
        found
    };
    timeout(Duration::from_secs(1), operation)
        .await
        .ok()
        .flatten()
}

/// Inspect only the current greeter under its own fixed UID.
///
/// # Errors
/// Rejects a different UID or an incomplete/stale greeter.
pub async fn probe_greeter() -> Result<GreeterStatus, ResourceControlError> {
    let account = session::account(Path::new("/"), "gdm")?;
    if rustix::process::geteuid().as_raw() != account.uid
        || rustix::process::getuid().as_raw() != account.uid
    {
        return Err(rejected("greeter probe requires the fixed gdm identity"));
    }
    let connection = Connection::system()
        .await
        .map_err(|_| unavailable("system bus is unavailable"))?;
    greeter_status(&connection)
        .await
        .ok_or_else(|| unavailable("greeter desktop is not yet available"))
}
