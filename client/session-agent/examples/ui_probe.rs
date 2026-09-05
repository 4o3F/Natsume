// Development-only probe harness; never packaged. Close hides non-mandatory
// screens while the process stays resident. A Binding prompt cannot be closed.
// Exit the probe with Ctrl-C. `ui_probe hidden` creates no window at all and
// parks in the event loop, demonstrating the same invariant.
use std::{
    env,
    ffi::OsString,
    io::{self, Write as _},
    process::ExitCode,
};

use natsume_local_control_api::{GraphicalSession, SessionScreenKind, SessionUiSnapshot};
use natsume_session_agent::ui;

fn parse_screen_kind(value: &str) -> Option<SessionScreenKind> {
    match value {
        "hidden" => Some(SessionScreenKind::Hidden),
        "binding_prompt" => Some(SessionScreenKind::BindingPrompt),
        "binding_pending" => Some(SessionScreenKind::BindingPending),
        _ => None,
    }
}

fn snapshot(screen: SessionScreenKind) -> SessionUiSnapshot {
    let binding_prompt = screen == SessionScreenKind::BindingPrompt;
    SessionUiSnapshot {
        session: GraphicalSession {
            logind_session_id: "ui-probe-session".to_owned(),
            boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        },
        ui_revision: 1,
        screen,
        binding_error_code: None,
        negotiation_id: binding_prompt.then(|| "probe-negotiation".to_owned()),
        submission_epoch: binding_prompt.then_some(1),
    }
}

fn argument() -> Result<SessionScreenKind, &'static str> {
    let mut arguments = env::args_os().skip(1);
    let Some(value) = arguments.next() else {
        return Err("usage: ui_probe <screen_kind>");
    };
    if arguments.next().is_some() {
        return Err("usage: ui_probe <screen_kind>");
    }
    let value = OsString::into_string(value).map_err(|_| "screen kind must be valid UTF-8")?;
    parse_screen_kind(&value).ok_or("unknown screen kind")
}

fn write_error(message: &str) {
    let _write_result = writeln!(io::stderr().lock(), "ui_probe: {message}");
}

fn main() -> ExitCode {
    // The probe observes the confirm/cancel round-trip through tracing lines
    // emitted by ui::apply's callbacks, so the subscriber must be installed.
    if tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .try_init()
        .is_err()
    {
        write_error("failed to initialize logging");
        return ExitCode::FAILURE;
    }
    let screen = match argument() {
        Ok(screen) => screen,
        Err(error) => {
            write_error(error);
            return ExitCode::from(2);
        }
    };
    // Same ambient-runtime requirement as the product binary: the winit
    // backend's XDG portal client is tokio-flavored zbus.
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            write_error(&format!("failed to start the tokio runtime: {error}"));
            return ExitCode::FAILURE;
        }
    };
    let _runtime_guard = runtime.enter();
    if let Err(error) = ui::apply(&snapshot(screen)) {
        write_error(&format!("failed to apply snapshot: {error}"));
        return ExitCode::FAILURE;
    }
    if let Err(error) = slint::run_event_loop_until_quit() {
        write_error(&format!("event loop failed: {error}"));
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
