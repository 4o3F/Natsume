#![forbid(unsafe_code)]

use std::{
    env,
    ffi::OsStr,
    io::{self, Write as _},
    process::ExitCode,
};

const LOGGING_FAILURE_ID: &str = "NATSUME_SESSION_AGENT_LOGGING_INIT_FAILED";
const EVENT_LOOP_FAILURE_REASON: &str = "slint_event_loop_failed";

enum RunError {
    Invocation(&'static str),
    Platform(slint::PlatformError),
    Runtime(io::Error),
}

fn run() -> Result<(), RunError> {
    let mut args = env::args_os().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some(mode), None) if mode == OsStr::new("--autostart") => {
            // Phase 0 resident process contract: desktop XDG Autostart owns
            // launch; the process begins hidden in the Slint event loop. The
            // probe provides lazy typed snapshot presentation; logind validation
            // and Daemon lease renewal remain owed to Phase 6 and must run
            // BEFORE the backend/event-loop initialization below (ADR-0035
            // orders session validation ahead of serving local UI).
            //
            // The workspace zbus is tokio-flavored, and Slint's winit backend
            // talks to the XDG desktop portal through zbus, so an ambient
            // tokio runtime must exist before the event loop starts or the
            // first portal call panics on the real desktop image.
            let runtime = tokio::runtime::Runtime::new().map_err(RunError::Runtime)?;
            let _runtime_guard = runtime.enter();
            // The residency marker must only fire once the loop is actually
            // pumping. invoke_from_event_loop rejects until the loop exists,
            // so a helper thread retries the enqueue with a bounded budget: on
            // platform failure the enqueue never succeeds and no false
            // "resident" is logged.
            std::thread::spawn(|| {
                for _ in 0..100 {
                    let queued = slint::invoke_from_event_loop(|| {
                        tracing::info!("session agent resident");
                    });
                    if queued.is_ok() {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            });
            slint::run_event_loop_until_quit().map_err(RunError::Platform)
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
        Err(RunError::Invocation(error)) => {
            tracing::error!(reason = error, "session agent startup rejected");
            ExitCode::from(2)
        }
        Err(RunError::Platform(error)) => {
            tracing::error!(
                reason = EVENT_LOOP_FAILURE_REASON,
                error = %error,
                "session agent event loop failed"
            );
            ExitCode::from(3)
        }
        Err(RunError::Runtime(error)) => {
            tracing::error!(
                reason = EVENT_LOOP_FAILURE_REASON,
                error = %error,
                "session agent runtime initialization failed"
            );
            ExitCode::from(3)
        }
    }
}
