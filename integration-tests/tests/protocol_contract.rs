const PROTO: &str = include_str!("../../crates/device-protocol/proto/device_control.proto");

fn message_body(name: &str) -> &'static str {
    let marker = format!("message {name} {{");
    let Some(start) = PROTO.find(&marker) else {
        panic!("message must exist: {name}");
    };
    let bytes = PROTO.as_bytes();
    let mut depth = 0usize;
    let mut body_start = None;

    for index in start..bytes.len() {
        match bytes[index] {
            b'{' => {
                depth += 1;
                if body_start.is_none() {
                    body_start = Some(index + 1);
                }
            }
            b'}' => {
                depth = match depth.checked_sub(1) {
                    Some(depth) => depth,
                    None => panic!("message braces must remain balanced: {name}"),
                };
                if depth == 0 {
                    let Some(body_start) = body_start else {
                        panic!("message body must start: {name}");
                    };
                    return &PROTO[body_start..index];
                }
            }
            _ => {}
        }
    }

    panic!("unterminated message {name}");
}

#[test]
fn protocol_uses_explicit_state_and_secret_commands() {
    assert!(PROTO.contains("message SyncState"));
    assert!(PROTO.contains("message SyncSecret"));
    assert!(PROTO.contains("message TargetStateSnapshot"));
    assert!(!PROTO.contains("GatewayCertificateMode gateway_certificate_mode"));
    assert!(!PROTO.contains("DesiredStateStatus"));
}

#[test]
fn protocol_has_no_installation_or_token_identity() {
    assert!(!PROTO.contains("installation_instance"));
    assert!(!PROTO.contains("bootstrap_token"));
    assert!(!PROTO.contains("CLONE_DETECTED"));
}

#[test]
fn binding_response_is_named_binding_result() {
    assert!(PROTO.contains("message BindingResult"));
    assert!(!PROTO.contains("BindingRequestResult"));
}

#[test]
fn enrollment_issues_device_token_and_gateway_certificate() {
    let request = message_body("EnrollDeviceRequest");
    let response = message_body("EnrollDeviceResponse");

    assert!(request.contains("string machine_hardware_id = 1;"));
    assert!(request.contains("HardwareClaim hardware_claim = 2;"));
    assert!(request.contains("bytes gateway_csr_der = 3;"));
    assert!(request.contains("string client_version = 4;"));
    assert!(request.contains("uint32 protocol_version = 5;"));
    assert!(!request.contains("device_identity_csr_der"));
    assert!(!request.contains("device_spki_sha256"));

    assert!(response.contains("string device_id = 1;"));
    assert!(response.contains("SecretBytes device_token = 2;"));
    assert!(response.contains("bytes gateway_leaf_der = 3;"));
    assert!(response.contains("repeated bytes gateway_chain_der = 4;"));
    assert!(response.contains("EnrollmentState state = 5;"));
    assert!(response.contains("string stable_error_code = 6;"));

    assert!(!PROTO.contains("message EnrollmentChallengeRequest"));
    assert!(!PROTO.contains("message EnrollmentChallengeResponse"));
    assert!(!PROTO.contains("message EnrollmentPollRequest"));
    assert!(!PROTO.contains("message EnrollmentPollResult"));
    assert!(!PROTO.contains("device_identity_csr_der"));
    assert!(!PROTO.contains("device_spki_sha256"));
    assert!(!PROTO.contains("device_leaf_der"));
    assert!(!PROTO.contains("device_chain_der"));
}

#[test]
fn gateway_certificate_is_issued_only_at_enrollment() {
    let enrollment_request = message_body("EnrollDeviceRequest");
    let enrollment_response = message_body("EnrollDeviceResponse");
    let sync_state = message_body("SyncState");
    let command = message_body("Command");

    assert!(enrollment_request.contains("gateway_csr_der"));
    assert!(enrollment_response.contains("gateway_leaf_der"));
    assert!(enrollment_response.contains("gateway_chain_der"));
    assert!(!PROTO.contains("message GatewayCertificateRequest"));
    assert!(!PROTO.contains("message GatewayCertificateResult"));
    assert!(!PROTO.contains("enum GatewayCertificateResultState"));
    assert!(!PROTO.contains("enum GatewayCertificateMode"));
    assert!(!PROTO.contains("STATE_APPLY_STATUS_WAITING_FOR_GATEWAY_CERTIFICATE"));
    assert!(!sync_state.contains("gateway_certificate"));
    assert!(!command.contains("gateway_certificate"));
    assert!(!command.contains("certificate"));
    assert!(!command.contains("install_certificate"));
    assert!(!PROTO.contains("message InstallCertificate"));
    assert!(!PROTO.contains("CertificateIssueRequest"));
    assert!(!PROTO.contains("CertificateIssueResult"));
}

#[test]
fn protocol_observes_cross_desktop_session_agent() {
    let observed = message_body("ObservedStateSnapshot");
    let agent = message_body("SessionAgentObservation");

    assert!(observed.contains("SessionAgentObservation session_agent"));
    assert!(agent.contains("GraphicalSessionType graphical_session_type"));
    assert!(agent.contains("DisplayBackend display_backend"));
    assert!(!agent.contains("reserved 4"));
    assert!(agent.contains("UiPresentationState presentation = 4;"));
    assert!(!PROTO.contains("enum SessionSupervisor"));
    assert!(agent.contains("UiPresentationState presentation"));
    assert!(agent.contains("SessionScreenKind screen"));
    assert!(PROTO.contains("UI_PRESENTATION_STATE_PRESENTED_UNFOCUSED"));
}

#[test]
fn revision_fields_are_numeric_and_field_numbers_are_stable() {
    let observed = message_body("ObservedStateSnapshot");
    assert!(observed.contains("uint64 installed_binding_revision = 8;"));
    assert!(observed.contains("uint64 installed_credential_revision = 9;"));
    assert!(observed.contains("uint64 gateway_configuration_revision = 12;"));
    assert!(!observed.contains("revision_id"));

    let assignment = message_body("TargetAssignment");
    assert!(assignment.contains("reserved 1, 5;"));
    assert!(assignment.contains("uint64 binding_revision = 2;"));
    assert!(assignment.contains("string seat_id = 3;"));
    assert!(assignment.contains("string seat_code = 4;"));
    assert!(assignment.contains("string account_id = 6;"));
    assert!(assignment.contains("string domjudge_username = 7;"));
    assert!(!assignment.contains("binding_id"));

    let gateway = message_body("TargetGateway");
    assert!(gateway.contains("uint64 gateway_configuration_revision = 1;"));
    assert!(!gateway.contains("uint64 configuration_revision = 1;"));
    assert!(!gateway.contains("configuration_revision_id"));

    let secret = message_body("SyncSecret");
    assert!(secret.contains("string seat_id = 1;"));
    assert!(secret.contains("uint64 binding_revision = 2;"));
    assert!(secret.contains("string account_id = 3;"));
    assert!(secret.contains("uint64 credential_revision = 4;"));
    assert!(secret.contains("SecretBytes password = 5;"));
    assert!(!secret.contains("seat_assignment_revision"));

    let binding_result = message_body("BindingResult");
    assert!(binding_result.contains("uint64 binding_revision = 3;"));
    assert!(!binding_result.contains("assignment_revision"));

    let command = message_body("Command");
    let status = message_body("CommandStatus");
    assert!(command.contains("string command_id = 1;"));
    assert!(status.contains("string command_id = 1;"));
}

#[test]
fn command_reserves_removed_surface() {
    let command = message_body("Command");

    assert!(command.contains("reserved 4, 5, 13, 14, 15, 16;"));
    assert!(command.contains(
        "reserved \"offline_policy\", \"resource_lane\", \"collect_diagnostics\", \"restart_agent\", \"run_local_preflight\", \"clear_local_secret\";"
    ));
    for removed_field in [
        "string offline_policy =",
        "string resource_lane =",
        "CollectDiagnostics collect_diagnostics =",
        "RestartAgent restart_agent =",
        "RunLocalPreflight run_local_preflight =",
        "ClearLocalSecret clear_local_secret =",
    ] {
        assert!(!command.contains(removed_field));
    }
    for removed_message in [
        "message CollectDiagnostics {",
        "message RestartAgent {",
        "message RunLocalPreflight {",
        "message ClearLocalSecret {",
    ] {
        assert!(!PROTO.contains(removed_message));
    }
}

#[test]
fn command_and_status_require_canonical_lowercase_uuidv7_ids() {
    use natsume_device_protocol::{
        generated::{
            Command, CommandState, CommandStatus, ControlEnvelope, LockSession, SessionTarget,
            command, control_envelope,
        },
        validate_envelope,
    };
    use natsume_error_code::{ErrorCode, control::ControlErrorCode};

    const UUID_V7: &str = "018f0e2e-8c1d-7c5e-8b12-3456789abcde";
    let command_envelope = |command_id: &str| ControlEnvelope {
        body: Some(control_envelope::Body::Command(Command {
            command_id: command_id.to_owned(),
            created_at_unix_ms: 0,
            deadline_unix_ms: 0,
            body: Some(command::Body::LockSession(LockSession {
                target: Some(SessionTarget {
                    session_instance_id: "session-1".to_owned(),
                    session_epoch: 1,
                }),
                requested_lock_epoch: 1,
            })),
        })),
    };
    let status_envelope = |command_id: &str| ControlEnvelope {
        body: Some(control_envelope::Body::CommandStatus(CommandStatus {
            command_id: command_id.to_owned(),
            state: CommandState::Received as i32,
            stable_error_code: String::new(),
            terminal_result_cursor: 0,
        })),
    };

    assert!(validate_envelope(&command_envelope(UUID_V7)).is_ok());
    assert!(validate_envelope(&status_envelope(UUID_V7)).is_ok());

    let uppercase = UUID_V7.to_uppercase();
    let braced = format!("{{{UUID_V7}}}");
    let compact = UUID_V7.replace('-', "");
    for invalid in [
        "550e8400-e29b-41d4-a716-446655440000",
        "018f0e2e-8c1d-7c5e-0b12-3456789abcde",
        "018f0e2e-8c1d-7c5e-cb12-3456789abcde",
        "018f0e2e-8c1d-7c5e-eb12-3456789abcde",
        uppercase.as_str(),
        braced.as_str(),
        compact.as_str(),
        "not-a-command-id",
    ] {
        for envelope in [command_envelope(invalid), status_envelope(invalid)] {
            let Err(error) = validate_envelope(&envelope) else {
                panic!("non-canonical command ID must fail: {invalid}");
            };
            assert_eq!(
                error.error_code(),
                ErrorCode::Control(ControlErrorCode::CommandIdInvalid)
            );
            let rendered = error.to_string();
            assert!(!rendered.contains(invalid));
        }
    }
}

#[test]
fn generated_descriptor_matches_the_committed_golden() {
    use prost::Message;

    let generated = natsume_device_protocol::file_descriptor_set();
    let golden = include_bytes!("../../crates/device-protocol/testdata/device_control.pb");
    assert_eq!(generated, golden);

    let Ok(descriptor) = prost_types::FileDescriptorSet::decode(generated) else {
        panic!("generated descriptor must decode");
    };
    assert_eq!(descriptor.file.len(), 1);
    assert_eq!(
        descriptor.file[0].package.as_deref(),
        Some("natsume.device.v2")
    );
    assert!(
        descriptor.file[0]
            .message_type
            .iter()
            .any(|message| message.name.as_deref() == Some("ControlEnvelope"))
    );
}

#[test]
fn semantic_validation_rejects_empty_oneofs_and_invalid_enums() {
    use natsume_device_protocol::{
        generated::{ControlEnvelope, Heartbeat, control_envelope},
        validate_envelope,
    };
    use natsume_error_code::{ErrorCode, control::ControlErrorCode};

    let Err(error) = validate_envelope(&ControlEnvelope { body: None }) else {
        panic!("empty envelope body must fail");
    };
    assert_eq!(
        error.error_code(),
        ErrorCode::Control(ControlErrorCode::ProtocolInvalidEnvelope)
    );

    let unspecified = ControlEnvelope {
        body: Some(control_envelope::Body::Heartbeat(Heartbeat::default())),
    };
    assert!(validate_envelope(&unspecified).is_err());

    let unknown = ControlEnvelope {
        body: Some(control_envelope::Body::Heartbeat(Heartbeat {
            session_lock_state: 999,
            ..Heartbeat::default()
        })),
    };
    assert!(validate_envelope(&unknown).is_err());
}
