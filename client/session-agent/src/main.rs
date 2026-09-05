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
        .ok_or(RunError::Identity("XDG_SESSION_ID is unavailable"))?;
    match env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland" | "x11") => {}
        _ => return Err(RunError::Identity("graphical session type is unsupported")),
    }
    Ok(GraphicalSession {
        logind_session_id,
        boot_id: canonical_boot_id()?,
    })
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

fn renew_after(lease: &SessionAgentLease) -> Duration {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX);
    let remaining = lease.expires_at_unix_ms.saturating_sub(now);
    Duration::from_millis(u64::try_from((remaining / 2).max(1_000)).unwrap_or(1_000))
}

fn hidden_snapshot(session: &GraphicalSession) -> SessionUiSnapshot {
    SessionUiSnapshot {
        session: session.clone(),
        ui_revision: 0,
        screen: SessionScreenKind::Hidden,
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
    let connection = zbus::Connection::system()
        .await
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
    ui::queue(initial);

    let mut refresh = tokio::time::interval(Duration::from_secs(1));
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let renewal = tokio::time::sleep(renew_after(&lease));
    tokio::pin!(renewal);

    loop {
        tokio::select! {
            () = &mut renewal => {
                lease = proxy
                    .renew_session_agent_lease(&lease.lease_id, session)
                    .await
                    .map_err(|_| ConnectionEnd::Disconnected("Session Agent lease renewal failed"))?;
                if lease.session != *session {
                    return Err(ConnectionEnd::Disconnected("Device1 renewed a different graphical session"));
                }
                renewal.as_mut().reset(Instant::now() + renew_after(&lease));
            }
            _ = refresh.tick() => {
                let snapshot = proxy
                    .get_session_ui_snapshot(&lease.lease_id, session)
                    .await
                    .map_err(|_| ConnectionEnd::Disconnected("Session UI snapshot refresh failed"))?;
                if snapshot.session != *session {
                    return Err(ConnectionEnd::Disconnected("Device1 returned a different graphical session"));
                }
                if snapshot.ui_revision > revision {
                    revision = snapshot.ui_revision;
                    ui::queue(snapshot);
                }
            }
            submission = submissions.recv() => {
                let Some(submission) = submission else {
                    return Err(ConnectionEnd::Disconnected("Binding submission channel closed"));
                };
                proxy.submit_binding(&lease.lease_id, &submission)
                    .await
                    .map_err(|_| ConnectionEnd::Disconnected("Binding submission failed"))?;
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
    session: GraphicalSession,
    mut submissions: mpsc::Receiver<BindingSubmission>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        match connected(&session, &mut submissions, &mut shutdown).await {
            Err(ConnectionEnd::Disconnected(reason)) => {
                tracing::warn!(reason, "Session Agent disconnected from Device1");
                ui::queue(hidden_snapshot(&session));
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
        (Some(mode), None) if mode == OsStr::new("--autostart") => {
            let session = session_identity()?;
            let runtime_directory = env::var_os("XDG_RUNTIME_DIR")
                .filter(|value| !value.is_empty())
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_absolute())
                .ok_or(RunError::Identity("XDG_RUNTIME_DIR is unavailable"))?;
            let _singleton_lock = singleton_lock(&runtime_directory).map_err(RunError::Runtime)?;
            let runtime = tokio::runtime::Runtime::new().map_err(RunError::Runtime)?;
            let runtime_guard = runtime.enter();
            let (submission_sender, submission_receiver) = mpsc::channel(1);
            let (shutdown_sender, shutdown_receiver) = watch::channel(false);
            ui::set_binding_submission_sender(submission_sender)
                .map_err(|_| RunError::Identity("Binding submission channel is duplicated"))?;
            let device_task =
                runtime.spawn(device_loop(session, submission_receiver, shutdown_receiver));

            std::thread::spawn(|| {
                for _ in 0..100 {
                    let queued = slint::invoke_from_event_loop(|| {
                        if let Err(error) = ui::apply_pending() {
                            tracing::error!(error = %error, "initial Session UI presentation failed");
                        }
                        tracing::info!("session agent resident");
                    });
                    if queued.is_ok() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            let event_loop_result = slint::run_event_loop_until_quit();
            let _shutdown_send_result = shutdown_sender.send(true);
            drop(runtime_guard);
            let _shutdown_result = runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(1), device_task).await
            });
            event_loop_result.map_err(RunError::Platform)
        }
        _ => Err(RunError::Invocation(
            "usage: natsume-session-agent --autostart",
        )),
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

    use super::{hidden_snapshot, singleton_lock};

    #[test]
    fn disconnect_snapshot_hides_the_exact_session_without_stale_binding_data() {
        let session = GraphicalSession {
            logind_session_id: "c2".to_owned(),
            boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        };
        let snapshot = hidden_snapshot(&session);

        assert_eq!(snapshot.session, session);
        assert_eq!(snapshot.screen, SessionScreenKind::Hidden);
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
