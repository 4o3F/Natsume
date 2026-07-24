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
    assert!(PROTO.contains("GatewayCertificateMode gateway_certificate_mode"));
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
fn enrollment_is_device_identity_only() {
    let request = message_body("EnrollDeviceRequest");
    let result = message_body("EnrollmentPollResult");

    assert!(request.contains("device_identity_csr_der"));
    assert!(request.contains("device_spki_sha256"));
    assert!(!request.contains("gateway"));

    assert!(result.contains("device_leaf_der"));
    assert!(result.contains("device_chain_der"));
    assert!(!result.contains("gateway"));
}

#[test]
fn gateway_certificate_is_a_sync_state_quic_subprotocol() {
    let request = message_body("GatewayCertificateRequest");
    let result = message_body("GatewayCertificateResult");
    let command = message_body("Command");

    for field in [
        "request_id",
        "command_id",
        "target_generation",
        "configuration_revision_id",
        "csr_der",
        "spki_sha256",
        "request_nonce",
    ] {
        assert!(
            request.contains(field),
            "missing gateway request field {field}"
        );
    }

    assert!(result.contains("GatewayCertificateResultState"));
    assert!(result.contains("certificate_fingerprint"));
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
    assert!(agent.contains("reserved 4"));
    assert!(!PROTO.contains("enum SessionSupervisor"));
    assert!(agent.contains("UiPresentationState presentation"));
    assert!(agent.contains("SessionScreenKind screen"));
    assert!(PROTO.contains("UI_PRESENTATION_STATE_PRESENTED_UNFOCUSED"));
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
fn framing_is_big_endian_bounded_and_streaming_safe() {
    use bytes::BytesMut;
    use natsume_device_protocol::{
        DEFAULT_MAX_FRAME_BYTES, decode_frame, encode_frame,
        generated::{ControlEnvelope, Heartbeat, control_envelope},
    };

    let envelope = ControlEnvelope {
        body: Some(control_envelope::Body::Heartbeat(Heartbeat::default())),
    };
    let Ok(frame) = encode_frame(&envelope, DEFAULT_MAX_FRAME_BYTES) else {
        panic!("small envelope must encode");
    };
    assert_eq!(
        u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize,
        frame.len() - 4
    );

    let mut incomplete = BytesMut::from(&frame[..frame.len() - 1]);
    assert!(matches!(
        decode_frame(&mut incomplete, DEFAULT_MAX_FRAME_BYTES),
        Ok(None)
    ));

    let mut complete = BytesMut::from(frame.as_ref());
    let Ok(Some(decoded)) = decode_frame(&mut complete, DEFAULT_MAX_FRAME_BYTES) else {
        panic!("complete frame must decode");
    };
    assert_eq!(decoded, envelope);
    assert!(complete.is_empty());
}

#[test]
fn framing_rejects_oversized_and_malformed_payloads() {
    use bytes::BytesMut;
    use natsume_device_protocol::decode_frame;
    use natsume_error_code::{AsErrorCode, ErrorCode};

    let mut oversized = BytesMut::from(&[0, 0, 0, 9][..]);
    let Err(error) = decode_frame(&mut oversized, 8) else {
        panic!("advertised oversize must fail before payload allocation");
    };
    assert_eq!(error.error_code(), ErrorCode::ProtocolFrameTooLarge);

    let mut malformed = BytesMut::from(&[0, 0, 0, 1, 0xff][..]);
    let Err(error) = decode_frame(&mut malformed, 8) else {
        panic!("malformed complete payload must fail");
    };
    assert_eq!(error.error_code(), ErrorCode::ProtocolInvalidEnvelope);
}

#[test]
fn semantic_validation_rejects_empty_oneofs_and_invalid_enums() {
    use natsume_device_protocol::{
        generated::{ControlEnvelope, Heartbeat, control_envelope},
        validate_envelope,
    };
    use natsume_error_code::{AsErrorCode, ErrorCode};

    let Err(error) = validate_envelope(&ControlEnvelope { body: None }) else {
        panic!("empty envelope body must fail");
    };
    assert_eq!(error.error_code(), ErrorCode::ProtocolInvalidEnvelope);

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
