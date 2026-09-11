#![forbid(unsafe_code)]

use std::{
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
    process::ExitCode,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use natsume_local_control_api::{
    BindingSubmission, Device1Proxy, GraphicalSession, SESSION_AGENT_SINGLETON_RELATIVE_PATH,
    SessionAgentLease, SessionScreenKind, SessionUiSnapshot,
};
use natsume_session_agent::ui;
use rustix::fs::{FlockOperation, flock};
use slint::winit_030::winit::platform::x11::EventLoopBuilderExtX11 as _;
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};
use uuid::Uuid;

const LOGGING_FAILURE_ID: &str = "NATSUME_SESSION_AGENT_LOGGING_INIT_FAILED";
const EVENT_LOOP_FAILURE_REASON: &str = "slint_event_loop_failed";
const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";

enum RunError {
    Invocation(&'static str),
    Identity(&'static str),
    Platform(slint::PlatformError),
    Runtime(io::Error),
}

enum ConnectionEnd {
    Disconnected(&'static str),
    Shutdown,
}

fn canonical_boot_id() -> Result<String, RunError> {
    let encoded = fs::read_to_string(BOOT_ID_PATH)
        .map_err(|_| RunError::Identity("boot identity is unavailable"))?;
    let encoded = encoded.trim();
    let boot_id =
        Uuid::parse_str(encoded).map_err(|_| RunError::Identity("boot identity is invalid"))?;
    let canonical = boot_id.hyphenated().to_string();
    if canonical != encoded {
        return Err(RunError::Identity("boot identity is invalid"));
    }
    Ok(canonical)
}

fn session_identity() -> Result<GraphicalSession, RunError> {
    let logind_session_id = env::var("XDG_SESSION_ID")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    match env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("x11") => {}
        _ => return Err(RunError::Identity("graphical session type is unsupported")),
    }
    Ok(GraphicalSession {
        logind_session_id,
        boot_id: canonical_boot_id()?,
    })
}

/// GNOME user services may run without `XDG_SESSION_ID`.
/// Resolve this process's display once; Device1 independently authenticates it.
/// Until then the local window needs no identity and cannot confirm readiness.
async fn resolve_session(session: &mut GraphicalSession) -> zbus::Result<()> {
    if !session.logind_session_id.is_empty() {
        return Ok(());
    }
    let connection = zbus::connection::Builder::system()?
        .method_timeout(Duration::from_secs(2))
        .build()
        .await?;
    let manager = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;
    let path: zbus::zvariant::OwnedObjectPath =
        manager.call("GetUserByPID", &(std::process::id(),)).await?;
    let user = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        path.as_str(),
        "org.freedesktop.login1.User",
    )
    .await?;
    let uid: u32 = user.get_property("UID").await?;
    let (display, _): (String, zbus::zvariant::OwnedObjectPath) =
        user.get_property("Display").await?;
    if uid != rustix::process::geteuid().as_raw() || display.is_empty() {
        return Err(zbus::Error::Failure(
            "waiting display is unavailable".into(),
        ));
    }
    session.logind_session_id = display;
    Ok(())
}

fn is_waiting_user() -> Result<bool, RunError> {
    let uid = rustix::process::geteuid().as_raw();
    let passwd = fs::read_to_string("/etc/passwd").map_err(RunError::Runtime)?;
    let mut users = passwd.lines().filter_map(|line| {
        let fields: Vec<_> = line.split(':').collect();
        (fields.len() == 7 && fields[2].parse::<u32>().ok() == Some(uid)).then_some(fields[0])
    });
    Ok(users.next() == Some("waiting") && users.next().is_none())
}

fn singleton_lock(runtime_directory: &Path) -> io::Result<File> {
    let lock_path = runtime_directory.join(SESSION_AGENT_SINGLETON_RELATIVE_PATH);
    let lock_directory = lock_path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "singleton path has no parent")
    })?;
    fs::create_dir_all(lock_directory)?;
    fs::set_permissions(lock_directory, fs::Permissions::from_mode(0o700))?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path)?;
    lock.set_permissions(fs::Permissions::from_mode(0o600))?;
    flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(io::Error::from)?;
    Ok(lock)
}

fn lease_deadline(lease: &SessionAgentLease) -> Instant {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX);
    Instant::now()
        + Duration::from_millis(
            u64::try_from(lease.expires_at_unix_ms.saturating_sub(now)).unwrap_or(0),
        )
}

async fn lease_call<T>(
    deadline: Instant,
    operation: impl std::future::Future<Output = zbus::Result<T>>,
) -> Result<T, ConnectionEnd> {
    tokio::time::timeout_at(deadline, operation)
        .await
        .map_err(|_| {
            ConnectionEnd::Disconnected("Session Agent lease expired during a local operation")
        })?
        .map_err(|_| ConnectionEnd::Disconnected("Device1 operation failed"))
}

fn renew_after(lease: &SessionAgentLease) -> Duration {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX);
    let remaining = lease.expires_at_unix_ms.saturating_sub(now);
    Duration::from_millis(u64::try_from((remaining / 2).max(1_000)).unwrap_or(1_000))
}

fn waiting_snapshot(session: &GraphicalSession) -> SessionUiSnapshot {
    SessionUiSnapshot {
        session: session.clone(),
        ui_revision: 0,
        screen: SessionScreenKind::Waiting,
        binding_error_code: None,
        negotiation_id: None,
        submission_epoch: None,
    }
}

async fn connected(
    session: &GraphicalSession,
    submissions: &mut mpsc::Receiver<BindingSubmission>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), ConnectionEnd> {
    let connection = tokio::time::timeout(Duration::from_secs(10), async {
        zbus::connection::Builder::system()?
            .method_timeout(Duration::from_secs(10))
            .build()
            .await
    })
    .await
    .map_err(|_| ConnectionEnd::Disconnected("system D-Bus timed out"))?
    .map_err(|_| ConnectionEnd::Disconnected("system D-Bus is unavailable"))?;
    let proxy = Device1Proxy::new(&connection)
        .await
        .map_err(|_| ConnectionEnd::Disconnected("Device1 is unavailable"))?;
    let (mut lease, initial) = proxy
        .register_session_agent(session)
        .await
        .map_err(|_| ConnectionEnd::Disconnected("Session Agent registration failed"))?;
    if lease.session != *session || initial.session != *session {
        return Err(ConnectionEnd::Disconnected(
            "Device1 returned a different graphical session",
        ));
    }
    let mut revision = initial.ui_revision;
    let mut presentations = ui::presentation_receiver();
    let mut deadline = lease_deadline(&lease);
    ui::queue(initial);

    let mut refresh = tokio::time::interval(Duration::from_secs(1));
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let renewal = tokio::time::sleep(renew_after(&lease));
    tokio::pin!(renewal);

    loop {
        tokio::select! {
            () = &mut renewal => {
                lease = lease_call(deadline, proxy.renew_session_agent_lease(&lease.lease_id, session)).await?;
                if lease.session != *session {
                    return Err(ConnectionEnd::Disconnected("Device1 renewed a different graphical session"));
                }
                deadline = lease_deadline(&lease);
                renewal.as_mut().reset(Instant::now() + renew_after(&lease));
            }
            _ = refresh.tick() => {
                let snapshot = lease_call(deadline, proxy.get_session_ui_snapshot(&lease.lease_id, session)).await?;
                if snapshot.session != *session {
                    return Err(ConnectionEnd::Disconnected("Device1 returned a different graphical session"));
                }
                if snapshot.ui_revision > revision {
                    revision = snapshot.ui_revision;
                    ui::queue(snapshot);
                } else {
                    ui::retry_presentation();
                }
            }
            submission = submissions.recv() => {
                let Some(submission) = submission else {
                    return Err(ConnectionEnd::Disconnected("Binding submission channel closed"));
                };
                lease_call(deadline, proxy.submit_binding(&lease.lease_id, &submission)).await?;
            }
            changed = presentations.changed() => {
                if changed.is_err() { return Err(ConnectionEnd::Disconnected("presentation channel closed")); }
                let frame = presentations.borrow_and_update().clone();
                if let Some(frame) = frame
                    && frame.session == *session && frame.ui_revision == revision
                {
                    lease_call(deadline, proxy.confirm_session_presentation(&lease.lease_id, &frame)).await?;
                }
            }
            () = tokio::time::sleep_until(deadline) => {
                return Err(ConnectionEnd::Disconnected("Session Agent lease expired"));
            }
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    return Err(ConnectionEnd::Shutdown);
                }
            }
        }
    }
}

async fn device_loop(
    mut session: GraphicalSession,
    mut submissions: mpsc::Receiver<BindingSubmission>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        let result = if matches!(
            tokio::time::timeout(Duration::from_secs(5), resolve_session(&mut session)).await,
            Ok(Ok(()))
        ) {
            connected(&session, &mut submissions, &mut shutdown).await
        } else {
            Err(ConnectionEnd::Disconnected(
                "waiting display is unavailable",
            ))
        };
        match result {
            Err(ConnectionEnd::Disconnected(reason)) => {
                tracing::warn!(reason, "Session Agent disconnected from Device1");
                ui::queue(waiting_snapshot(&session));
            }
            Err(ConnectionEnd::Shutdown) => return,
            Ok(()) => {}
        }
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(1)) => {}
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

fn run() -> Result<(), RunError> {
    let mut args = env::args_os().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some(mode), None) if mode == OsStr::new("run") => {
            if !is_waiting_user()? {
                return Ok(());
            }
            let runtime = tokio::runtime::Runtime::new().map_err(RunError::Runtime)?;
            let session = session_identity()?;
            let runtime_directory = env::var_os("XDG_RUNTIME_DIR")
                .filter(|value| !value.is_empty())
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_absolute())
                .ok_or(RunError::Identity("XDG_RUNTIME_DIR is unavailable"))?;
            let _singleton_lock = singleton_lock(&runtime_directory).map_err(RunError::Runtime)?;
            let runtime_guard = runtime.enter();
            let (submission_sender, submission_receiver) = mpsc::channel(1);
            let (shutdown_sender, shutdown_receiver) = watch::channel(false);
            ui::set_binding_submission_sender(submission_sender)
                .map_err(|_| RunError::Identity("Binding submission channel is duplicated"))?;
            let mut event_loop = slint::winit_030::winit::event_loop::EventLoop::with_user_event();
            event_loop.with_x11();
            slint::BackendSelector::new()
                .backend_name("winit".into())
                .renderer_name("skia".into())
                .with_winit_event_loop_builder(event_loop)
                .select()
                .map_err(RunError::Platform)?;
            ui::apply(&waiting_snapshot(&session)).map_err(RunError::Platform)?;
            let device_task = runtime.spawn(device_loop(
                session.clone(),
                submission_receiver,
                shutdown_receiver,
            ));

            let event_loop_result = slint::run_event_loop_until_quit();
            let _shutdown_send_result = shutdown_sender.send(true);
            drop(runtime_guard);
            let _shutdown_result = runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(1), device_task).await
            });
            event_loop_result.map_err(RunError::Platform)
        }
        _ => Err(RunError::Invocation("usage: natsume-session-agent run")),
    }
}

fn initialize_logging() -> Result<(), ()> {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .try_init()
        .map_err(|_| ())
}

fn main() -> ExitCode {
    if initialize_logging().is_err() {
        let _write_result = writeln!(io::stderr().lock(), "{LOGGING_FAILURE_ID}");
        return ExitCode::FAILURE;
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(RunError::Invocation(error) | RunError::Identity(error)) => {
            tracing::error!(reason = error, "session agent startup rejected");
            ExitCode::from(2)
        }
        Err(RunError::Platform(error)) => {
            tracing::error!(reason = EVENT_LOOP_FAILURE_REASON, error = %error, "session agent event loop failed");
            ExitCode::from(3)
        }
        Err(RunError::Runtime(error)) => {
            tracing::error!(reason = EVENT_LOOP_FAILURE_REASON, error = %error, "session agent runtime initialization failed");
            ExitCode::from(3)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use natsume_local_control_api::{
        GraphicalSession, SESSION_AGENT_SINGLETON_RELATIVE_PATH, SessionScreenKind,
    };
    use tempfile::TempDir;

    use super::{singleton_lock, waiting_snapshot};

    #[tokio::test]
    async fn an_unresponsive_local_call_cannot_outlive_the_lease() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(10);
        let result = super::lease_call(deadline, std::future::pending::<zbus::Result<()>>()).await;
        assert!(matches!(result, Err(super::ConnectionEnd::Disconnected(_))));
    }

    #[test]
    fn disconnect_snapshot_keeps_waiting_without_stale_binding_data() {
        let session = GraphicalSession {
            logind_session_id: "c2".to_owned(),
            boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        };
        let snapshot = waiting_snapshot(&session);

        assert_eq!(snapshot.session, session);
        assert_eq!(snapshot.screen, SessionScreenKind::Waiting);
        assert!(snapshot.binding_error_code.is_none());
        assert!(snapshot.negotiation_id.is_none());
        assert!(snapshot.submission_epoch.is_none());
    }

    #[test]
    fn singleton_lock_is_owner_only_and_exclusive() {
        let runtime = TempDir::new()
            .unwrap_or_else(|error| panic!("runtime fixture creation failed: {error}"));
        let first = singleton_lock(runtime.path())
            .unwrap_or_else(|error| panic!("first singleton lock failed: {error}"));
        assert!(singleton_lock(runtime.path()).is_err());

        let lock_path = runtime.path().join(SESSION_AGENT_SINGLETON_RELATIVE_PATH);
        let directory_mode = lock_path
            .parent()
            .unwrap_or_else(|| panic!("lock path must have a parent"))
            .metadata()
            .unwrap_or_else(|error| panic!("lock directory metadata failed: {error}"))
            .permissions()
            .mode();
        let file_mode = lock_path
            .metadata()
            .unwrap_or_else(|error| panic!("lock metadata failed: {error}"))
            .permissions()
            .mode();
        assert_eq!(directory_mode & 0o777, 0o700);
        assert_eq!(file_mode & 0o777, 0o600);

        drop(first);
        singleton_lock(runtime.path())
            .unwrap_or_else(|error| panic!("released singleton lock failed: {error}"));
    }
}
