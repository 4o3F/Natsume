use std::{
    cell::RefCell,
    sync::{LazyLock, Mutex, OnceLock},
};

use natsume_local_control_api::{
    BindingSubmission, SessionPresentation, SessionScreenKind, SessionUiSnapshot,
};
use slint::{ComponentHandle as _, winit_030::WinitWindowAccessor as _};
use tokio::sync::{mpsc::Sender, watch};

// Slint owns this generated surface. First-party source remains subject to the
// workspace lint policy; generated implementation details are isolated here.
mod generated {
    #![allow(
        unsafe_code,
        clippy::all,
        clippy::pedantic,
        clippy::expect_used,
        clippy::unwrap_used
    )]

    slint::include_modules!();
}

pub use generated::SessionWindow;

thread_local! {
    static WINDOW: RefCell<Option<SessionWindow>> = const { RefCell::new(None) };
    static CURRENT_SNAPSHOT: RefCell<Option<SessionUiSnapshot>> = const { RefCell::new(None) };
}

static PENDING_SNAPSHOT: Mutex<Option<SessionUiSnapshot>> = Mutex::new(None);
static BINDING_SUBMISSIONS: OnceLock<Sender<BindingSubmission>> = OnceLock::new();

static PRESENTED: LazyLock<watch::Sender<Option<SessionPresentation>>> =
    LazyLock::new(|| watch::channel(None).0);

/// Watches frames rendered at the actual native monitor size, on the UI thread.
#[must_use]
pub fn presentation_receiver() -> watch::Receiver<Option<SessionPresentation>> {
    PRESENTED.subscribe()
}

/// Revalidates native geometry and retries after monitor/window events settle.
/// Healthy static pages do not redraw on the periodic Device1 refresh.
pub fn retry_presentation() {
    let _queued = slint::invoke_from_event_loop(|| {
        WINDOW.with(|window| {
            if let Some(window) = window.borrow().as_ref() {
                let observed = frame_geometry(window);
                if observed
                    .as_ref()
                    .is_some_and(|frame| frame.first_frame_presented)
                    && *PRESENTED.borrow() == observed
                {
                    return;
                }
                // Geometry alone cannot confirm a newly sized surface. Revoke
                // the old frame now; only AfterRendering can confirm it again.
                let invalid = observed.map(|mut frame| {
                    frame.first_frame_presented = false;
                    frame.fullscreen_width = 0;
                    frame.fullscreen_height = 0;
                    frame
                });
                publish_frame(invalid);
                window.window().request_redraw();
            }
        });
    });
}

fn frame_geometry(window: &SessionWindow) -> Option<SessionPresentation> {
    let fullscreen = window
        .window()
        .with_winit_window(|native| {
            native.fullscreen().is_some()
                // On X11 current_monitor retains the window's last monitor
                // snapshot when RandR changes size without changing DPI.
                && native.available_monitors().any(|monitor| {
                    native.inner_size() == monitor.size()
                        && native.outer_position().ok() == Some(monitor.position())
                })
        })
        .unwrap_or(false);
    let size = window.window().size();
    CURRENT_SNAPSHOT.with(|current| {
        current
            .borrow()
            .as_ref()
            .map(|snapshot| SessionPresentation {
                agent_lease_id: snapshot.agent_lease_id.clone(),
                session: snapshot.session.clone(),
                ui_revision: snapshot.ui_revision,
                first_frame_presented: fullscreen && size.width > 0 && size.height > 0,
                fullscreen_width: if fullscreen { size.width } else { 0 },
                fullscreen_height: if fullscreen { size.height } else { 0 },
            })
    })
}

fn publish_frame(frame: Option<SessionPresentation>) {
    PRESENTED.send_if_modified(|current| {
        if *current == frame {
            false
        } else {
            *current = frame;
            true
        }
    });
}

fn record_frame(window: &SessionWindow) {
    let frame = frame_geometry(window);
    // AfterRendering precedes the backend's buffer presentation. Publish on
    // the next UI event-loop turn, and discard a revision superseded meanwhile.
    let _queued = slint::invoke_from_event_loop(move || {
        let current_revision = CURRENT_SNAPSHOT.with(|current| {
            current.borrow().as_ref().map(|snapshot| {
                (
                    snapshot.session.clone(),
                    snapshot.ui_revision,
                    snapshot.agent_lease_id.clone(),
                )
            })
        });
        if frame.as_ref().map(|frame| {
            (
                frame.session.clone(),
                frame.ui_revision,
                frame.agent_lease_id.clone(),
            )
        }) == current_revision
        {
            publish_frame(frame);
        }
    });
}

#[must_use]
pub fn seat_input_visible(snapshot: &SessionUiSnapshot) -> bool {
    !snapshot.offline
        && snapshot.screen == SessionScreenKind::BindingPrompt
        && snapshot.negotiation_id.is_some()
        && snapshot.submission_epoch.is_some_and(|epoch| epoch != 0)
}

fn snapshot_text(snapshot: &SessionUiSnapshot) -> (&'static str, &'static str, &'static str) {
    match snapshot.screen {
        SessionScreenKind::Waiting => ("", "", ""),
        SessionScreenKind::BindingPrompt => (
            "绑定座位",
            "Bind your seat",
            "输入座位号，关联对应队伍。\n绑定后将显示队伍信息。",
        ),
        SessionScreenKind::BindingPending => (
            "正在绑定座位",
            "Binding your seat",
            "正在确认座位信息，请稍候。\n确认后将显示对应队伍信息。",
        ),
    }
}

fn binding_error_text(code: Option<&str>) -> &'static str {
    match code {
        None => "",
        Some("SEAT_NOT_FOUND") => "未找到这个座位号，请检查后重试。",
        Some("SEAT_UNMAPPED") => "该座位尚未关联队伍，请联系现场工作人员。",
        Some("SEAT_OCCUPIED") => "该座位已被占用。\n请核对座位号，或联系现场工作人员。",
        Some(_) => "暂时无法绑定座位，请重试或联系现场工作人员。",
    }
}

fn binding_submission(
    snapshot: &SessionUiSnapshot,
    seat_code: String,
) -> Option<BindingSubmission> {
    if !seat_input_visible(snapshot) || seat_code.is_empty() {
        return None;
    }
    Some(BindingSubmission {
        session: snapshot.session.clone(),
        negotiation_id: snapshot.negotiation_id.clone()?,
        submission_epoch: snapshot.submission_epoch.filter(|epoch| *epoch != 0)?,
        seat_code,
    })
}

/// Installs the sole channel used by UI callbacks to submit Binding input.
///
/// # Errors
///
/// Returns the sender when a channel was already installed.
pub fn set_binding_submission_sender(
    sender: Sender<BindingSubmission>,
) -> Result<(), Sender<BindingSubmission>> {
    BINDING_SUBMISSIONS.set(sender)
}

/// Stores the latest Daemon snapshot and asks the Slint thread to apply it.
pub fn queue(snapshot: SessionUiSnapshot) {
    match PENDING_SNAPSHOT.lock() {
        Ok(mut pending) => *pending = Some(snapshot),
        Err(poisoned) => *poisoned.into_inner() = Some(snapshot),
    }
    let _queue_result = slint::invoke_from_event_loop(|| {
        if let Err(error) = apply_pending() {
            tracing::error!(error = %error, "Session Agent presentation failed");
        }
    });
}

/// Applies a queued snapshot after the Slint event loop becomes available.
///
/// # Errors
///
/// Returns a platform error when window creation or presentation fails.
pub fn apply_pending() -> Result<(), slint::PlatformError> {
    let snapshot = match PENDING_SNAPSHOT.lock() {
        Ok(mut pending) => pending.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    match snapshot {
        Some(snapshot) => apply(&snapshot),
        None => Ok(()),
    }
}

/// Applies one typed Daemon snapshot to the lazily created Session Agent window.
///
/// Must be called on the Slint event-loop thread: the window handle lives in a
/// thread-local slot and Slint window operations are not thread-safe.
///
/// # Errors
///
/// Returns a platform error when window creation, visibility, or presentation fails.
pub fn apply(snapshot: &SessionUiSnapshot) -> Result<(), slint::PlatformError> {
    CURRENT_SNAPSHOT.with(|current| *current.borrow_mut() = Some(snapshot.clone()));
    PRESENTED.send_replace(None);
    let existing =
        WINDOW.with_borrow(|slot| slot.as_ref().map(slint::ComponentHandle::clone_strong));

    let window = if let Some(window) = existing {
        window
    } else {
        let window = SessionWindow::new()?;
        let placeholder = std::path::Path::new("/usr/share/natsume/waiting.png");
        if placeholder.is_file()
            && let Ok(logo) = slint::Image::load_from_path(placeholder)
        {
            window.set_waiting_logo(logo);
        }
        let weak = window.as_weak();
        window
            .window()
            .set_rendering_notifier(move |state, _| {
                if matches!(state, slint::RenderingState::AfterRendering)
                    && let Some(window) = weak.upgrade()
                {
                    record_frame(&window);
                }
            })
            .map_err(|error| slint::PlatformError::Other(error.to_string()))?;
        window
            .window()
            .on_close_requested(|| slint::CloseRequestResponse::KeepWindowShown);
        window.on_confirm_seat({
            let weak = window.as_weak();
            move |seat_code| {
                let Some(window) = weak.upgrade() else {
                    tracing::warn!(
                        reason = "session_window_gone",
                        "seat confirmation raced window teardown"
                    );
                    return;
                };
                let submission = CURRENT_SNAPSHOT.with(|current| {
                    current
                        .borrow()
                        .as_ref()
                        .and_then(|snapshot| binding_submission(snapshot, seat_code.to_string()))
                });
                let Some(submission) = submission else {
                    tracing::warn!("Binding confirmation has no current intent");
                    return;
                };
                let Some(sender) = BINDING_SUBMISSIONS.get() else {
                    tracing::warn!("Binding submission channel is unavailable");
                    return;
                };
                if sender.try_send(submission).is_err() {
                    tracing::warn!("Binding submission is already pending or unavailable");
                    return;
                }
                window.set_seat_code(String::new().into());
            }
        });
        WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone_strong()));
        window
    };

    window.set_team_visible(snapshot.team.is_some());
    window.set_offline(snapshot.offline);
    // Reset the logo before resolving a new binding; a previous school is never a fallback.
    let mut logo = slint::Image::load_from_svg_data(include_bytes!("../ui/school.svg"))
        .map_err(|error| slint::PlatformError::Other(error.to_string()))?;
    if let Some(team) = &snapshot.team {
        let (school, school_secondary) = display_names(&team.school_name_zh, &team.school_name_en);
        let (name, name_secondary) = display_names(&team.team_name_zh, &team.team_name_en);
        window.set_school_primary(school.into());
        window.set_school_secondary(school_secondary.into());
        window.set_long_team(name.chars().count() > 30);
        window.set_long_seat(team.seat_code.chars().count() > 12);
        window.set_team_primary(name.into());
        window.set_team_secondary(name_secondary.into());
        window.set_assigned_seat(team.seat_code.as_str().into());
        if let Some(path) = &snapshot.logo_path {
            match slint::Image::load_from_path(std::path::Path::new(path)) {
                Ok(image) => logo = image,
                Err(error) => {
                    tracing::warn!(%error, "School logo unavailable; displaying default icon");
                }
            }
        }
    }
    window.set_school_logo(logo);
    let (title, subtitle, message) = snapshot_text(snapshot);
    window.set_waiting_visible(snapshot.screen == SessionScreenKind::Waiting);
    window.window().set_fullscreen(true);
    window.set_binding_pending(snapshot.screen == SessionScreenKind::BindingPending);
    window
        .set_binding_error_text(binding_error_text(snapshot.binding_error_code.as_deref()).into());
    window.set_subtitle_text(subtitle.into());
    window.set_title_text(title.into());
    window.set_message_text(message.into());
    window.set_seat_input_visible(seat_input_visible(snapshot));
    window.show()?;
    window.window().request_redraw();
    Ok(())
}

fn display_names<'a>(primary: &'a str, secondary: &'a str) -> (&'a str, &'a str) {
    if primary.is_empty() {
        (secondary, "")
    } else {
        (primary, secondary)
    }
}

#[cfg(test)]
mod tests {
    use natsume_local_control_api::{GraphicalSession, SessionScreenKind, SessionUiSnapshot};

    use super::{binding_error_text, binding_submission, seat_input_visible, snapshot_text};

    fn snapshot(screen: SessionScreenKind) -> SessionUiSnapshot {
        SessionUiSnapshot {
            agent_lease_id: String::new(),
            session: GraphicalSession {
                logind_session_id: "test-session".to_owned(),
                boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
            },
            ui_revision: 1,
            screen,
            team: None,
            logo_path: None,
            offline: false,
            binding_error_code: None,
            negotiation_id: None,
            submission_epoch: None,
        }
    }

    #[test]
    fn offline_prompts_are_never_actionable_and_single_language_is_not_duplicated() {
        let mut prompt = snapshot(SessionScreenKind::BindingPrompt);
        prompt.negotiation_id = Some("negotiation".into());
        prompt.submission_epoch = Some(1);
        prompt.offline = true;
        assert!(!seat_input_visible(&prompt));
        assert!(binding_submission(&prompt, "A-01".into()).is_none());
        assert_eq!(super::display_names("", "Team"), ("Team", ""));
        assert_eq!(super::display_names("队伍", "Team"), ("队伍", "Team"));
    }

    #[test]
    fn binding_copy_names_the_seat_and_errors_do_not_expose_protocol_codes() {
        let (title, subtitle, message) = snapshot_text(&snapshot(SessionScreenKind::BindingPrompt));
        assert_eq!(title, "绑定座位");
        assert_eq!(subtitle, "Bind your seat");
        assert!(!message.contains("工位"));
        let (title, subtitle, _) = snapshot_text(&snapshot(SessionScreenKind::BindingPending));
        assert_eq!(title, "正在绑定座位");
        assert_eq!(subtitle, "Binding your seat");
        for code in [
            "SEAT_NOT_FOUND",
            "SEAT_UNMAPPED",
            "SEAT_OCCUPIED",
            "FUTURE_ERROR",
        ] {
            let text = binding_error_text(Some(code));
            assert!(!text.is_empty());
            assert!(!text.contains(code));
            assert!(!text.contains("工位"));
        }
        assert_eq!(binding_error_text(None), "");
    }

    #[test]
    fn seat_code_is_visible_only_for_a_complete_binding_intent() {
        let mut prompt = snapshot(SessionScreenKind::BindingPrompt);
        prompt.negotiation_id = Some("019c1234-5678-7abc-8def-0123456789ab".to_owned());
        prompt.submission_epoch = Some(1);
        assert!(seat_input_visible(&prompt));
        assert!(!seat_input_visible(&snapshot(
            SessionScreenKind::BindingPrompt
        )));
        prompt.screen = SessionScreenKind::BindingPending;
        assert!(!seat_input_visible(&prompt));
    }

    #[test]
    fn binding_confirmation_echoes_the_current_intent_generation() {
        let mut prompt = snapshot(SessionScreenKind::BindingPrompt);
        prompt.negotiation_id = Some("019c1234-5678-7abc-8def-0123456789ab".to_owned());
        prompt.submission_epoch = Some(3);

        assert!(binding_submission(&prompt, String::new()).is_none());
        let submission = binding_submission(&prompt, "A-01".to_owned())
            .unwrap_or_else(|| panic!("complete Binding intent must submit"));
        assert_eq!(submission.session, prompt.session);
        assert_eq!(
            submission.negotiation_id,
            "019c1234-5678-7abc-8def-0123456789ab"
        );
        assert_eq!(submission.submission_epoch, 3);
        assert_eq!(submission.seat_code, "A-01");
    }
}
