const SCHEMA: &str = include_str!("../../server/migrations/0001_initial.sql");

fn table_body(name: &str) -> &'static str {
    let marker = format!("CREATE TABLE {name} (");
    let Some(start) = SCHEMA.find(&marker) else {
        panic!("table {name} must exist");
    };
    let rest = &SCHEMA[start + marker.len()..];
    let Some(end) = rest.find("\n) STRICT;") else {
        panic!("table {name} must end with STRICT");
    };
    &rest[..end]
}

#[test]
fn enrollment_table_is_device_identity_only() {
    let enrollment = table_body("enrollment_requests");

    assert!(enrollment.contains("device_csr_der"));
    assert!(enrollment.contains("device_spki_sha256"));
    assert!(!enrollment.contains("gateway_csr"));
    assert!(!enrollment.contains("gateway_spki"));
}

#[test]
fn automation_has_no_certificate_or_secret_side_effect_switches() {
    let policy = table_body("automation_policy_revisions");

    assert!(policy.contains("auto_approve_enrollment"));
    assert!(policy.contains("auto_sync_state_after_binding"));
    assert!(!policy.contains("auto_issue_device_certificate"));
    assert!(!policy.contains("auto_issue_gateway_certificate"));
    assert!(!policy.contains("auto_sync_secret"));
}

#[test]
fn gateway_certificate_request_is_bound_to_sync_state_identity() {
    let request = table_body("gateway_certificate_requests");

    for field in [
        "command_id",
        "device_pk",
        "target_generation",
        "configuration_revision_id",
        "csr_der",
        "spki_sha256",
        "request_nonce_sha256",
    ] {
        assert!(request.contains(field), "missing request field {field}");
    }

    assert!(SCHEMA.contains("issued_for_command_id TEXT NOT NULL REFERENCES commands(command_id)"));
    assert!(SCHEMA.contains("CREATE UNIQUE INDEX one_active_gateway_certificate"));
}
