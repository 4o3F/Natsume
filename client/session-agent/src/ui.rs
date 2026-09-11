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
            current
                .borrow()
                .as_ref()
                .map(|snapshot| (snapshot.session.clone(), snapshot.ui_revision))
        });
        if frame
            .as_ref()
            .map(|frame| (frame.session.clone(), frame.ui_revision))
            == current_revision
        {
            publish_frame(frame);
        }
    });
}

#[must_use]
pub fn seat_input_visible(snapshot: &SessionUiSnapshot) -> bool {
    snapshot.screen == SessionScreenKind::BindingPrompt
        && snapshot.negotiation_id.is_some()
        && snapshot.submission_epoch.is_some_and(|epoch| epoch != 0)
}

#[must_use]
pub const fn screen_kind_label(kind: SessionScreenKind) -> &'static str {
    match kind {
        SessionScreenKind::Waiting => "waiting",
        SessionScreenKind::BindingPrompt => "binding_prompt",
        SessionScreenKind::BindingPending => "binding_pending",
    }
}

fn snapshot_text(snapshot: &SessionUiSnapshot) -> (String, String) {
    let (title, message) = match snapshot.screen {
        SessionScreenKind::Waiting => ("", ""),
        SessionScreenKind::BindingPrompt => ("Bind workstation", "Enter your seat code"),
        SessionScreenKind::BindingPending => ("Binding workstation", "Waiting for the server"),
    };
    let message = snapshot.binding_error_code.as_ref().map_or_else(
        || message.to_owned(),
        |error_code| format!("{message}\n{error_code}"),
    );
    (title.to_owned(), message)
}

fn binding_submission(
    snapshot: &SessionUiSnapshot,
    seat_code: String,
) -> Option<BindingSubmission> {
    if snapshot.screen != SessionScreenKind::BindingPrompt {
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
    let existing = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(slint::ComponentHandle::clone_strong)
    });

    let window = if let Some(window) = existing {
        window
    } else {
        let window = SessionWindow::new()?;
        if let Ok(logo) =
            slint::Image::load_from_path(std::path::Path::new("/usr/share/natsume/waiting.png"))
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

    let (title, message) = snapshot_text(snapshot);
    window.set_waiting_visible(snapshot.screen == SessionScreenKind::Waiting);
    window.window().set_fullscreen(true);
    window.set_screen_kind_text(screen_kind_label(snapshot.screen).into());
    window.set_title_text(title.into());
    window.set_message_text(message.into());
    window.set_seat_input_visible(seat_input_visible(snapshot));
    window.show()?;
    window.window().request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use natsume_local_control_api::{GraphicalSession, SessionScreenKind, SessionUiSnapshot};

    use super::{binding_submission, screen_kind_label, seat_input_visible, snapshot_text};

    fn snapshot(screen: SessionScreenKind) -> SessionUiSnapshot {
        SessionUiSnapshot {
            session: GraphicalSession {
                logind_session_id: "test-session".to_owned(),
                boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
            },
            ui_revision: 1,
            screen,
            binding_error_code: None,
            negotiation_id: None,
            submission_epoch: None,
        }
    }

    #[test]
    fn snapshot_text_uses_the_selected_screen() {
        let probe = snapshot(SessionScreenKind::BindingPending);
        let (title, message) = snapshot_text(&probe);
        assert_eq!(title, "Binding workstation");
        assert_eq!(message, "Waiting for the server");
    }

    #[test]
    fn snapshot_text_includes_the_current_binding_error() {
        let mut probe = snapshot(SessionScreenKind::BindingPrompt);
        probe.binding_error_code = Some("SEAT_OCCUPIED".to_owned());
        let (title, message) = snapshot_text(&probe);
        assert_eq!(title, "Bind workstation");
        assert_eq!(message, "Enter your seat code\nSEAT_OCCUPIED");
    }

    #[test]
    fn screen_kind_labels_cover_the_typed_contract() {
        let cases = [
            (SessionScreenKind::Waiting, "waiting"),
            (SessionScreenKind::BindingPrompt, "binding_prompt"),
            (SessionScreenKind::BindingPending, "binding_pending"),
        ];

        for (kind, expected) in cases {
            assert_eq!(screen_kind_label(kind), expected);
        }
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
