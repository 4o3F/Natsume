// Development-only probe harness; never packaged. All screens remain visible
// when closing is requested. Exit the probe with Ctrl-C.
// `ui_probe waiting` displays the black waiting placeholder.
use std::{env, ffi::OsString, process::ExitCode};

use natsume_local_control_api::{GraphicalSession, SessionScreenKind, SessionUiSnapshot};
use natsume_session_agent::ui;

fn parse_screen_kind(value: &str) -> Option<SessionScreenKind> {
    match value {
        "waiting" | "waiting_team" | "waiting_offline" | "waiting_long" => {
            Some(SessionScreenKind::Waiting)
        }
        "binding_prompt" | "binding_error" | "binding_roundtrip" => {
            Some(SessionScreenKind::BindingPrompt)
        }
        "binding_pending" => Some(SessionScreenKind::BindingPending),
        _ => None,
    }
}

fn snapshot(screen: SessionScreenKind, mode: &str) -> SessionUiSnapshot {
    let binding_prompt = screen == SessionScreenKind::BindingPrompt;
    let mut snapshot = SessionUiSnapshot {
        agent_lease_id: String::new(),
        session: GraphicalSession {
            logind_session_id: "ui-probe-session".to_owned(),
            boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        },
        ui_revision: 1,
        screen,
        team: None,
        logo_path: None,
        offline: false,
        binding_error_code: None,
        negotiation_id: binding_prompt.then(|| "probe-negotiation".to_owned()),
        submission_epoch: binding_prompt.then_some(1),
    };
    if mode == "binding_error" {
        snapshot.binding_error_code = Some("SEAT_OCCUPIED".into());
    }
    if matches!(mode, "waiting_team" | "waiting_offline" | "waiting_long") {
        let long = mode == "waiting_long";
        snapshot.team = Some(natsume_local_control_api::WaitingTeam {
            binding_id: "01900000-0000-7000-8000-000000000001".into(),
            account_id: "01900000-0000-7000-8000-000000000002".into(),
            seat_code: "A-108".into(),
            organization_id: "INST-001".into(),
            team_name_zh: if long {
                "一个需要换行且必须完整展示名称的程序设计竞赛队伍".repeat(4)
            } else {
                "星河漫游".into()
            },
            team_name_en: if long {
                "An Exceptionally Long Team Name That Must Remain Fully Readable ".repeat(4)
            } else {
                "Voyagers of the Stars".into()
            },
            school_name_zh: "三峡大学".into(),
            school_name_en: "China Three Gorges University".into(),
        });
        snapshot.logo_path = env::var("NATSUME_UI_PROBE_LOGO").ok();
        snapshot.offline = mode == "waiting_offline";
    }
    snapshot
}

fn argument() -> Result<(SessionScreenKind, String), &'static str> {
    let mut arguments = env::args_os().skip(1);
    let Some(value) = arguments.next() else {
        return Err("usage: ui_probe <screen_kind>");
    };
    if arguments.next().is_some() {
        return Err("usage: ui_probe <screen_kind>");
    }
    let value = OsString::into_string(value).map_err(|_| "screen kind must be valid UTF-8")?;
    parse_screen_kind(&value)
        .map(|screen| (screen, value))
        .ok_or("unknown screen kind")
}

fn write_error(message: &str) {
    tracing::error!(message, "UI probe failed");
}

fn main() -> ExitCode {
    // The probe observes the confirm round-trip through tracing lines
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
    let (screen, mode) = match argument() {
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
    let (sender, mut submissions) = tokio::sync::mpsc::channel(1);
    if ui::set_binding_submission_sender(sender).is_err() {
        write_error("submission channel was already installed");
        return ExitCode::FAILURE;
    }
    let roundtrip = mode == "binding_roundtrip";
    runtime.spawn(async move {
        while let Some(submission) = submissions.recv().await {
            tracing::info!(
                seat_code = submission.seat_code,
                submission_epoch = submission.submission_epoch,
                negotiation_id = submission.negotiation_id,
                "Probe binding submission"
            );
            if roundtrip {
                let mut pending = snapshot(SessionScreenKind::BindingPending, "binding_pending");
                pending.ui_revision = submission.submission_epoch.saturating_mul(2);
                ui::queue(pending);
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let mut rejected = snapshot(SessionScreenKind::BindingPrompt, "binding_error");
                rejected.ui_revision = submission
                    .submission_epoch
                    .saturating_mul(2)
                    .saturating_add(1);
                rejected.submission_epoch = Some(submission.submission_epoch.saturating_add(1));
                ui::queue(rejected);
            }
        }
    });
    if let Err(error) = ui::apply(&snapshot(screen, &mode)) {
        write_error(&format!("failed to apply snapshot: {error}"));
        return ExitCode::FAILURE;
    }
    let mut frames = ui::presentation_receiver();
    runtime.spawn(async move {
        while frames.changed().await.is_ok() {
            if let Some(frame) = frames.borrow_and_update().as_ref() {
                tracing::info!(
                    revision = frame.ui_revision,
                    ready = frame.first_frame_presented,
                    width = frame.fullscreen_width,
                    height = frame.fullscreen_height,
                    "Probe frame"
                );
            }
        }
    });
    if let Err(error) = slint::run_event_loop_until_quit() {
        write_error(&format!("event loop failed: {error}"));
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
