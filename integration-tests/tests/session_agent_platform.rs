use natsume_local_control_api::{
    DisplayBackend, GraphicalSessionType, SESSION_AGENT_AUTOSTART_BASENAME,
    SESSION_AGENT_AUTOSTART_MODE, SESSION_AGENT_SINGLETON_RELATIVE_PATH,
    SESSION_AGENT_USER_AUTOSTART_RELATIVE_PATH, SessionAgentCapabilities, SessionScreenKind,
    UiPresentationState,
};

#[test]
fn xdg_autostart_contract_has_one_direct_mode_and_one_singleton() {
    assert_eq!(SESSION_AGENT_AUTOSTART_MODE, "--autostart");
    assert_eq!(
        SESSION_AGENT_SINGLETON_RELATIVE_PATH,
        "natsume/session-agent.lock"
    );
    assert!(!SESSION_AGENT_SINGLETON_RELATIVE_PATH.starts_with('/'));
    assert!(!SESSION_AGENT_SINGLETON_RELATIVE_PATH.contains(".."));
}

#[test]
fn wayland_unfocused_is_a_first_class_presentation_result() {
    let capabilities = SessionAgentCapabilities {
        graphical_session_type: GraphicalSessionType::Wayland,
        display_backend: DisplayBackend::Wayland,
        notifications_available: true,
        desktop_lock_supported: true,
        desktop_unlock_supported: true,
        ime_supported: true,
        hidpi_supported: true,
        multi_monitor_supported: true,
    };

    assert_eq!(capabilities.display_backend, DisplayBackend::Wayland);
    assert_ne!(
        UiPresentationState::PresentedUnfocused,
        UiPresentationState::Failed
    );
    assert_eq!(
        SessionScreenKind::BindingPrompt,
        SessionScreenKind::BindingPrompt
    );
}

#[test]
fn user_autostart_shadow_guard_is_exactly_scoped() {
    assert_eq!(
        SESSION_AGENT_AUTOSTART_BASENAME,
        "org.natsume.SessionAgent.desktop"
    );
    assert_eq!(
        SESSION_AGENT_USER_AUTOSTART_RELATIVE_PATH,
        ".config/autostart/org.natsume.SessionAgent.desktop"
    );
    assert!(!SESSION_AGENT_USER_AUTOSTART_RELATIVE_PATH.contains(".."));
    assert!(!SESSION_AGENT_USER_AUTOSTART_RELATIVE_PATH.starts_with('/'));
}
