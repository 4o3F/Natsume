use std::cell::RefCell;

use natsume_local_control_api::{SeatInputPolicy, SessionScreenKind, SessionUiSnapshot};
use slint::ComponentHandle as _;

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
}

#[must_use]
pub fn seat_input_visible(snapshot: &SessionUiSnapshot) -> bool {
    snapshot.screen == SessionScreenKind::BindingPrompt
        && snapshot.seat_input_policy == Some(SeatInputPolicy::SeatCode)
}

#[must_use]
pub const fn screen_kind_label(kind: SessionScreenKind) -> &'static str {
    match kind {
        SessionScreenKind::Hidden => "hidden",
        SessionScreenKind::IdleStatus => "idle_status",
        SessionScreenKind::BindingPrompt => "binding_prompt",
        SessionScreenKind::BindingPending => "binding_pending",
        SessionScreenKind::BindingResult => "binding_result",
        SessionScreenKind::RecoveryStatus => "recovery_status",
        SessionScreenKind::LockPresentation => "lock_presentation",
        SessionScreenKind::FatalLocalError => "fatal_local_error",
    }
}

fn snapshot_text(snapshot: &SessionUiSnapshot) -> (String, String) {
    let title = snapshot
        .parameters
        .iter()
        .find(|parameter| parameter.name == "title")
        .map_or_else(
            || snapshot.message_id.clone(),
            |parameter| parameter.value.clone(),
        );
    let details = snapshot
        .parameters
        .iter()
        .filter(|parameter| parameter.name != "title")
        .map(|parameter| format!("{}: {}", parameter.name, parameter.value))
        .collect::<Vec<_>>()
        .join("\n");
    let message = if details.is_empty() {
        snapshot.message_id.clone()
    } else {
        format!("{}\n{details}", snapshot.message_id)
    };
    (title, message)
}

/// Applies one typed Daemon snapshot to the lazily created Session Agent window.
///
/// Must be called on the Slint event-loop thread: the window handle lives in a
/// thread-local slot and Slint window operations are not thread-safe. The slot
/// borrow is released before every Slint call, so window callbacks may re-enter
/// `apply` without a `RefCell` borrow panic.
///
/// # Errors
///
/// Returns a platform error when window creation, visibility, or presentation fails.
pub fn apply(snapshot: &SessionUiSnapshot) -> Result<(), slint::PlatformError> {
    let existing = WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(slint::ComponentHandle::clone_strong)
    });

    if snapshot.screen == SessionScreenKind::Hidden {
        if let Some(window) = existing {
            window.hide()?;
        }
        return Ok(());
    }

    let window = if let Some(window) = existing {
        window
    } else {
        let window = SessionWindow::new()?;
        window.on_cancel({
            let weak = window.as_weak();
            move || {
                if let Some(window) = weak.upgrade() {
                    if let Err(error) = window.hide() {
                        tracing::error!(
                            reason = "session_window_hide_failed",
                            error = %error,
                            "session window cancellation failed"
                        );
                    }
                } else {
                    tracing::warn!(
                        reason = "session_window_gone",
                        "cancellation raced window teardown"
                    );
                }
            }
        });
        window.on_confirm_seat({
            let weak = window.as_weak();
            move |seat_code| {
                if weak.upgrade().is_none() {
                    tracing::warn!(
                        reason = "session_window_gone",
                        "seat confirmation raced window teardown"
                    );
                    return;
                }
                // Probe round-trip only; Phase 6 wires the typed D-Bus submission.
                tracing::info!(seat_code = %seat_code, "seat code confirmed");
            }
        });
        WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone_strong()));
        window
    };

    let (title, message) = snapshot_text(snapshot);
    window.set_screen_kind_text(screen_kind_label(snapshot.screen).into());
    window.set_title_text(title.into());
    window.set_message_text(message.into());
    window.set_seat_input_visible(seat_input_visible(snapshot));
    window.show()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use natsume_local_control_api::{
        SeatInputPolicy, SessionScreenKind, SessionTarget, SessionUiSnapshot, UiPresentationState,
        UiTextParameter,
    };

    use super::{screen_kind_label, seat_input_visible, snapshot_text};

    fn snapshot(
        screen: SessionScreenKind,
        seat_input_policy: Option<SeatInputPolicy>,
    ) -> SessionUiSnapshot {
        SessionUiSnapshot {
            schema_version: 1,
            target: SessionTarget {
                session_instance_id: "test-session".to_owned(),
                session_epoch: 1,
            },
            ui_revision: 1,
            screen,
            message_id: "test.message".to_owned(),
            parameters: Vec::new(),
            machine_short_id: "machine".to_owned(),
            seat_label: None,
            prompt_command_id: None,
            prompt_nonce: None,
            expires_at_unix_ms: None,
            seat_input_policy,
            presentation: UiPresentationState::Hidden,
        }
    }

    #[test]
    fn snapshot_text_falls_back_to_the_message_id_without_a_title_parameter() {
        let probe = snapshot(SessionScreenKind::IdleStatus, None);
        let (title, message) = snapshot_text(&probe);
        assert_eq!(title, "test.message");
        assert_eq!(message, "test.message");
    }

    #[test]
    fn snapshot_text_uses_the_first_title_and_passes_cjk_through_untouched() {
        let mut probe = snapshot(SessionScreenKind::BindingPrompt, None);
        probe.parameters = vec![
            UiTextParameter {
                name: "title".to_owned(),
                value: "中文渲染样例".to_owned(),
            },
            UiTextParameter {
                name: "title".to_owned(),
                value: "second-title".to_owned(),
            },
            UiTextParameter {
                name: "seat".to_owned(),
                value: "座位 A-01".to_owned(),
            },
        ];
        let (title, message) = snapshot_text(&probe);
        assert_eq!(title, "中文渲染样例");
        assert_eq!(message, "test.message\nseat: 座位 A-01");
    }

    #[test]
    fn screen_kind_labels_cover_the_typed_contract() {
        let cases = [
            (SessionScreenKind::Hidden, "hidden"),
            (SessionScreenKind::IdleStatus, "idle_status"),
            (SessionScreenKind::BindingPrompt, "binding_prompt"),
            (SessionScreenKind::BindingPending, "binding_pending"),
            (SessionScreenKind::BindingResult, "binding_result"),
            (SessionScreenKind::RecoveryStatus, "recovery_status"),
            (SessionScreenKind::LockPresentation, "lock_presentation"),
            (SessionScreenKind::FatalLocalError, "fatal_local_error"),
        ];

        for (kind, expected) in cases {
            assert_eq!(screen_kind_label(kind), expected);
        }
    }

    #[test]
    fn seat_code_is_visible_only_for_the_binding_prompt_policy() {
        assert!(seat_input_visible(&snapshot(
            SessionScreenKind::BindingPrompt,
            Some(SeatInputPolicy::SeatCode),
        )));
        assert!(!seat_input_visible(&snapshot(
            SessionScreenKind::BindingPrompt,
            Some(SeatInputPolicy::OperatorSelectedSeat),
        )));
        assert!(!seat_input_visible(&snapshot(
            SessionScreenKind::BindingPrompt,
            None,
        )));
        assert!(!seat_input_visible(&snapshot(
            SessionScreenKind::BindingPending,
            Some(SeatInputPolicy::SeatCode),
        )));
    }
}
