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

fn parse_xml(xml: &str) -> roxmltree::Document<'_> {
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    let Ok(document) = roxmltree::Document::parse_with_options(xml, options) else {
        panic!("D-Bus XML must parse");
    };
    document
}

fn xml_method_names(xml: &str) -> std::collections::BTreeSet<String> {
    parse_xml(xml)
        .descendants()
        .filter(|node| node.has_tag_name("method"))
        .filter_map(|node| node.attribute("name"))
        .map(str::to_owned)
        .collect()
}

fn xml_method_output_signatures(xml: &str, method_name: &str) -> Vec<String> {
    let document = parse_xml(xml);
    let Some(method) = document
        .descendants()
        .find(|node| node.has_tag_name("method") && node.attribute("name") == Some(method_name))
    else {
        panic!("D-Bus method must exist: {method_name}");
    };
    method
        .children()
        .filter(|node| node.has_tag_name("arg") && node.attribute("direction") == Some("out"))
        .filter_map(|node| node.attribute("type"))
        .map(str::to_owned)
        .collect()
}

fn assert_xml_contains_signature<T: zbus::zvariant::Type>(xml: &str) {
    let signature = T::SIGNATURE.to_string();
    assert!(
        xml.contains(&format!("type=\"{signature}\"")),
        "XML must contain Rust value signature {signature}"
    );
}

#[test]
fn device1_introspection_matches_the_typed_session_agent_contract() {
    use natsume_local_control_api::{
        BindingSubmission, DEVICE1_INTERFACE, DEVICE1_INTROSPECTION_XML, DEVICE1_PATH,
        DEVICE1_SERVICE, SessionAgentLease, SessionAgentRegistration, SessionTarget,
        SessionUiAction, SessionUiSnapshot, UiPresentationAck,
    };

    let expected = std::collections::BTreeSet::from([
        "AcknowledgePresentation".to_owned(),
        "GetSessionUiSnapshot".to_owned(),
        "RegisterSessionAgent".to_owned(),
        "RenewSessionAgentLease".to_owned(),
        "SubmitBinding".to_owned(),
        "SubmitSessionUiAction".to_owned(),
        "UnregisterSessionAgent".to_owned(),
    ]);
    assert_eq!(xml_method_names(DEVICE1_INTROSPECTION_XML), expected);
    assert_eq!(
        xml_method_output_signatures(DEVICE1_INTROSPECTION_XML, "RenewSessionAgentLease"),
        vec![<SessionAgentLease as zbus::zvariant::Type>::SIGNATURE.to_string()]
    );
    assert!(DEVICE1_INTROSPECTION_XML.contains(DEVICE1_INTERFACE));
    assert_eq!(DEVICE1_SERVICE, DEVICE1_INTERFACE);
    assert!(DEVICE1_INTROSPECTION_XML.contains(DEVICE1_PATH));
    assert!(!DEVICE1_INTROSPECTION_XML.contains("Caddy"));
    assert!(!DEVICE1_INTROSPECTION_XML.contains("Vault"));

    assert_xml_contains_signature::<SessionAgentRegistration>(DEVICE1_INTROSPECTION_XML);
    assert_xml_contains_signature::<SessionAgentLease>(DEVICE1_INTROSPECTION_XML);
    assert_xml_contains_signature::<SessionUiSnapshot>(DEVICE1_INTROSPECTION_XML);
    assert_xml_contains_signature::<SessionTarget>(DEVICE1_INTROSPECTION_XML);
    assert_xml_contains_signature::<SessionUiAction>(DEVICE1_INTROSPECTION_XML);
    assert_xml_contains_signature::<BindingSubmission>(DEVICE1_INTROSPECTION_XML);
    assert_xml_contains_signature::<UiPresentationAck>(DEVICE1_INTROSPECTION_XML);
}

#[test]
fn privileged1_introspection_and_package_policies_are_consistent() {
    use natsume_local_control_api::{
        ApplyLockRequest, ApplyUnlockRequest, PRIVILEGED1_INTERFACE, PRIVILEGED1_INTROSPECTION_XML,
        PRIVILEGED1_PATH, PRIVILEGED1_SERVICE, SanitizedHardwareClaim, SessionControlApplied,
        SessionTarget,
    };

    let expected = std::collections::BTreeSet::from([
        "ActivateHomeInstance".to_owned(),
        "CollectHardwareCandidates".to_owned(),
        "GarbageCollectHomeInstance".to_owned(),
        "InstallManagedBrowserPolicy".to_owned(),
        "PrepareHomeInstance".to_owned(),
        "QueryContestSession".to_owned(),
        "RecoverHomeInstance".to_owned(),
        "RequestDesktopLock".to_owned(),
        "RequestDesktopUnlock".to_owned(),
        "TerminateContestSession".to_owned(),
    ]);
    assert_eq!(xml_method_names(PRIVILEGED1_INTROSPECTION_XML), expected);
    assert!(PRIVILEGED1_INTROSPECTION_XML.contains(PRIVILEGED1_INTERFACE));
    assert_eq!(PRIVILEGED1_SERVICE, PRIVILEGED1_INTERFACE);
    assert!(PRIVILEGED1_INTROSPECTION_XML.contains(PRIVILEGED1_PATH));

    assert_xml_contains_signature::<SanitizedHardwareClaim>(PRIVILEGED1_INTROSPECTION_XML);
    assert_xml_contains_signature::<SessionTarget>(PRIVILEGED1_INTROSPECTION_XML);
    assert_xml_contains_signature::<ApplyLockRequest>(PRIVILEGED1_INTROSPECTION_XML);
    assert_xml_contains_signature::<ApplyUnlockRequest>(PRIVILEGED1_INTROSPECTION_XML);
    assert_xml_contains_signature::<SessionControlApplied>(PRIVILEGED1_INTROSPECTION_XML);

    let device_policy = include_str!(
        "../../packaging/client/rootfs/usr/share/dbus-1/system.d/org.natsume.Device1.conf"
    );
    let privileged_policy = include_str!(
        "../../packaging/client/rootfs/usr/share/dbus-1/system.d/org.natsume.Privileged1.conf"
    );
    let _device_policy_methods = xml_method_names(device_policy);
    let _privileged_policy_methods = xml_method_names(privileged_policy);
    assert!(device_policy.contains("user=\"natsume\"><allow own=\"org.natsume.Device1\""));
    assert!(
        device_policy.contains("user=\"contest\"><allow send_destination=\"org.natsume.Device1\"")
    );
    assert!(privileged_policy.contains("user=\"root\"><allow own=\"org.natsume.Privileged1\""));
    assert!(
        privileged_policy
            .contains("user=\"natsume\"><allow send_destination=\"org.natsume.Privileged1\"")
    );
}
