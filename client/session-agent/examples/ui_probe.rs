// Development-only probe harness; never packaged. Close/Cancel hides the
// window while the process stays resident (product semantics for the lazy
// window contract) — exit the probe with Ctrl-C. `ui_probe hidden` creates no
// window at all and parks in the event loop, demonstrating the same invariant.
use std::{
    env,
    ffi::OsString,
    io::{self, Write as _},
    process::ExitCode,
};

use natsume_local_control_api::{
    SeatInputPolicy, SessionScreenKind, SessionTarget, SessionUiSnapshot, UiPresentationState,
    UiTextParameter,
};
use natsume_session_agent::ui;

fn parse_screen_kind(value: &str) -> Option<SessionScreenKind> {
    match value {
        "hidden" => Some(SessionScreenKind::Hidden),
        "idle_status" => Some(SessionScreenKind::IdleStatus),
        "binding_prompt" => Some(SessionScreenKind::BindingPrompt),
        "binding_pending" => Some(SessionScreenKind::BindingPending),
        "binding_result" => Some(SessionScreenKind::BindingResult),
        "recovery_status" => Some(SessionScreenKind::RecoveryStatus),
        "lock_presentation" => Some(SessionScreenKind::LockPresentation),
        "fatal_local_error" => Some(SessionScreenKind::FatalLocalError),
        _ => None,
    }
}

fn parameters(screen: SessionScreenKind) -> Vec<UiTextParameter> {
    if screen == SessionScreenKind::BindingPrompt {
        vec![
            UiTextParameter {
                name: "title".to_owned(),
                value: "中文渲染样例".to_owned(),
            },
            UiTextParameter {
                name: "message".to_owned(),
                value: "请输入座位 A-01、混排 CJK+Latin，并检查 HiDPI。".to_owned(),
            },
        ]
    } else {
        vec![UiTextParameter {
            name: "message".to_owned(),
            value: format!("Session Agent probe: {}", ui::screen_kind_label(screen)),
        }]
    }
}

fn snapshot(screen: SessionScreenKind) -> SessionUiSnapshot {
    let binding_prompt = screen == SessionScreenKind::BindingPrompt;
    SessionUiSnapshot {
        schema_version: 1,
        target: SessionTarget {
            session_instance_id: "ui-probe-session".to_owned(),
            session_epoch: 1,
        },
        ui_revision: 1,
        screen,
        message_id: format!("session_agent.probe.{}", ui::screen_kind_label(screen)),
        parameters: parameters(screen),
        machine_short_id: "VM-PROBE".to_owned(),
        seat_label: binding_prompt.then(|| "A-01".to_owned()),
        prompt_command_id: binding_prompt.then(|| "probe-command".to_owned()),
        prompt_nonce: binding_prompt.then(|| "probe-nonce".to_owned()),
        expires_at_unix_ms: binding_prompt.then_some(4_102_444_800_000),
        seat_input_policy: binding_prompt.then_some(SeatInputPolicy::SeatCode),
        presentation: if screen == SessionScreenKind::Hidden {
            UiPresentationState::Hidden
        } else {
            UiPresentationState::Presenting
        },
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
