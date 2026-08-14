#![forbid(unsafe_code)]

use std::{
    env,
    ffi::OsStr,
    io::{self, Write as _},
    process::ExitCode,
    thread,
};

const LOGGING_FAILURE_ID: &str = "NATSUME_SESSION_AGENT_LOGGING_INIT_FAILED";

fn run() -> Result<(), &'static str> {
    let mut args = env::args_os().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some(mode), None) if mode == OsStr::new("--autostart") => {
            // Phase 0 resident process contract: desktop XDG Autostart owns
            // launch; the process begins hidden and does not create a window.
            // The desktop-capability probe still owes logind validation, Daemon
            // lease renewal and the minimal lazy Slint slice; Phase 6 owns the
            // full GUI state machines.
            loop {
                thread::park();
            }
        }
        _ => Err("usage: natsume-session-agent --autostart"),
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
        Err(error) => {
            tracing::error!(reason = error, "session agent startup rejected");
            ExitCode::from(2)
        }
    }
}
