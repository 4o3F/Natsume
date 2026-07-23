const PROTO: &str = include_str!("../../crates/device-protocol/proto/device_control.proto");

fn message_body(name: &str) -> &'static str {
    let marker = format!("message {name} {{");
    let Some(start) = PROTO.find(&marker) else {
        panic!("message {name} must exist");
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
                assert!(depth > 0, "message {name} has unbalanced braces");
                depth -= 1;
                if depth == 0 {
                    let Some(body_start) = body_start else {
                        panic!("message {name} body must start before it ends");
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
