use serde_json::{Value, value::RawValue};

use super::types::{CommandError, CommandKind, CommandRequestInput};
use super::validate::{fingerprint_v1, validate_payload, validate_request};

const DEVICE_ID: &str = "01900000-0000-7000-8000-000000000101";
const SEAT_ID: &str = "550e8400-e29b-41d4-a716-446655440001";
const ACCOUNT_ID: &str = "550e8400-e29b-41d4-a716-446655440002";
const LOCK_COMMAND_ID: &str = "01900000-0000-7000-8000-000000000103";
const LOWER_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn fingerprint_v1_matches_three_independent_golden_vectors() {
    let payload = raw(
        r#"{"target":{"session_instance_id":"session-a","session_epoch":42},"requested_lock_epoch":43}"#,
    );
    let minimal = validated(CommandRequestInput {
        device_id: DEVICE_ID.to_owned(),
        kind: "lock_session".to_owned(),
        payload_version: 1,
        payload,
        reason_code: None,
        group_correlation_id: None,
    });
    // Exact JCS input: {"device_id":"01900000-0000-7000-8000-000000000101","kind":"lock_session","payload":{"requested_lock_epoch":43,"target":{"session_epoch":42,"session_instance_id":"session-a"}},"payload_version":1}
    assert_eq!(
        hex::encode(&minimal.fingerprint.sha256),
        "d2685957cd9ab30ce0bf2832340e31b6d9a1ed9590e99ee2af38684e57bb8a70"
    );
    assert_eq!(
        minimal.frozen_payload_json,
        r#"{"requested_lock_epoch":43,"target":{"session_epoch":42,"session_instance_id":"session-a"}}"#
    );
    assert_eq!(minimal.device_id.as_text(), DEVICE_ID);

    let optional = validated(CommandRequestInput {
        device_id: DEVICE_ID.to_owned(),
        kind: "lock_session".to_owned(),
        payload_version: 1,
        payload: raw(
            r#"{"target":{"session_instance_id":"session-a","session_epoch":42},"requested_lock_epoch":43}"#,
        ),
        reason_code: Some("operator_requested".to_owned()),
        group_correlation_id: Some("550e8400-e29b-41d4-a716-446655440000".to_owned()),
    });
    // Exact JCS input: {"device_id":"01900000-0000-7000-8000-000000000101","group_correlation_id":"550e8400-e29b-41d4-a716-446655440000","kind":"lock_session","payload":{"requested_lock_epoch":43,"target":{"session_epoch":42,"session_instance_id":"session-a"}},"payload_version":1,"reason_code":"operator_requested"}
    assert_eq!(
        hex::encode(&optional.fingerprint.sha256),
        "818fb2fd3b30ec84850e7321e1960394fa5b93465d9c458ef9d31c68ccc590e7"
    );
    assert_eq!(optional.frozen_payload_json, minimal.frozen_payload_json);

    // This primitive-only vector is intentionally outside the printable-ASCII payload schema.
    let unicode_payload = serde_json::json!({
        "expires_at_unix_ms": 1_700_000_000_000_i64,
        "prompt_message_id": "競賽☃\n\""
    });
    let unicode_fingerprint = fingerprint(
        "01900000-0000-7000-8000-000000000102",
        CommandKind::OpenBindingPrompt,
        &unicode_payload,
        None,
        None,
    );
    // Exact JCS input: {"device_id":"01900000-0000-7000-8000-000000000102","kind":"open_binding_prompt","payload":{"expires_at_unix_ms":1700000000000,"prompt_message_id":"競賽☃\n\""},"payload_version":1}
    assert_eq!(
        hex::encode(unicode_fingerprint),
        "055330df9dd272fd27a44663ab7307e3c3d93f2688b4bb65a4dc02d8cf2f281a"
    );
}

#[test]
fn payload_schema_v1_accepts_all_seven_command_kinds() {
    let cases = [
        (CommandKind::SyncState, sync_state_payload()),
        (
            CommandKind::SyncSecret,
            format!(
                r#"{{"seat_id":"{SEAT_ID}","binding_revision":1,"account_id":"{ACCOUNT_ID}","credential_revision":2}}"#
            ),
        ),
        (
            CommandKind::OpenBindingPrompt,
            r#"{"expires_at_unix_ms":1700000000000,"prompt_message_id":"prompt-1"}"#
                .to_owned(),
        ),
        (
            CommandKind::LockSession,
            r#"{"target":{"session_instance_id":"session-1","session_epoch":2},"requested_lock_epoch":3}"#
                .to_owned(),
        ),
        (
            CommandKind::UnlockSession,
            format!(
                r#"{{"target":{{"session_instance_id":"session-1","session_epoch":2}},"expected_lock_epoch":3,"expected_lock_command_id":"{LOCK_COMMAND_ID}"}}"#
            ),
        ),
        (
            CommandKind::TerminateSession,
            r#"{"target":{"session_instance_id":"session-1","session_epoch":2}}"#.to_owned(),
        ),
        (
            CommandKind::ResetHome,
            r#"{"home_template_revision":"home-v1","home_epoch":4}"#.to_owned(),
        ),
    ];

    for (kind, payload) in cases {
        assert!(
            validate_payload(kind, raw(&payload).as_ref()).is_ok(),
            "a valid payload schema projection was rejected"
        );
    }
}

#[test]
fn payload_schema_v1_rejects_closed_shape_type_and_bound_violations() {
    let oversized = "x".repeat(129);
    let cases = [
        (
            CommandKind::LockSession,
            r#"{"target":{"session_instance_id":"session-1","session_epoch":2},"requested_lock_epoch":3,"unknown":true}"#.to_owned(),
        ),
        (
            CommandKind::LockSession,
            r#"{"target":{"session_instance_id":"session-1","session_epoch":2}}"#.to_owned(),
        ),
        (
            CommandKind::ResetHome,
            r#"{"home_template_revision":"home-v1","home_epoch":9007199254740992}"#.to_owned(),
        ),
        (
            CommandKind::OpenBindingPrompt,
            r#"{"expires_at_unix_ms":9007199254740992,"prompt_message_id":"prompt-1"}"#.to_owned(),
        ),
        (
            CommandKind::SyncState,
            sync_state_payload().replacen("\"schema_version\":1", "\"schema_version\":0", 1),
        ),
        (
            CommandKind::LockSession,
            r#"{"target":{"session_instance_id":"session-1","session_epoch":2},"requested_lock_epoch":1.5}"#.to_owned(),
        ),
        (
            CommandKind::SyncState,
            sync_state_payload().replace(LOWER_HASH, &"A".repeat(64)),
        ),
        (
            CommandKind::SyncState,
            sync_state_payload().replace(LOWER_HASH, &"a".repeat(63)),
        ),
        (
            CommandKind::SyncSecret,
            format!(
                r#"{{"seat_id":"550E8400-E29B-41D4-A716-446655440001","binding_revision":1,"account_id":"{ACCOUNT_ID}","credential_revision":2}}"#
            ),
        ),
        (
            CommandKind::SyncState,
            sync_state_payload().replacen("\"generation\":1", "\"generation\":1,\"generation\":2", 1),
        ),
        (
            CommandKind::ResetHome,
            format!(r#"{{"home_template_revision":"{oversized}","home_epoch":4}}"#),
        ),
        (
            CommandKind::SyncSecret,
            format!(
                r#"{{"seat_id":"{SEAT_ID}","binding_revision":1,"account_id":"{ACCOUNT_ID}","credential_revision":2,"password":"never-store-this"}}"#
            ),
        ),
        (
            CommandKind::OpenBindingPrompt,
            r#"{"expires_at_unix_ms":1700000000000,"prompt_message_id":"競賽"}"#.to_owned(),
        ),
    ];

    for (kind, payload) in cases {
        assert_payload_invalid(kind, &payload);
    }
}

#[test]
fn request_schema_v1_rejects_wrong_version_and_noncanonical_request_fields() {
    let wrong_version = validate_request(lock_request(2, None, None, DEVICE_ID));
    assert_error(&wrong_version, CommandError::PayloadInvalid);

    let bad_reason = validate_request(lock_request(1, Some("contains space"), None, DEVICE_ID));
    assert_error(&bad_reason, CommandError::ReasonCodeInvalid);

    let bad_group = validate_request(lock_request(1, None, Some("NOT-A-UUID"), DEVICE_ID));
    assert_error(&bad_group, CommandError::GroupCorrelationIdInvalid);

    let bad_device = validate_request(lock_request(
        1,
        None,
        None,
        "01900000000070008000000000000101",
    ));
    assert_error(&bad_device, CommandError::DeviceIdInvalid);
}

fn sync_state_payload() -> String {
    format!(
        r#"{{"generation":1,"canonical_hash":"{LOWER_HASH}","snapshot":{{"schema_version":1,"assignment":{{"binding_revision":1,"seat_id":"{SEAT_ID}","seat_code":"A-01","account_id":"{ACCOUNT_ID}","domjudge_username":"team-a"}},"gateway":{{"gateway_configuration_revision":1,"local_origin_hostname":"device.local","fixed_upstream_profile_id":"upstream-v1","exact_login_policy_id":"login-v1","gateway_certificate_profile_id":"gateway-v1","gateway_certificate_min_valid_until_unix_ms":1700000000000}},"session":{{"browser_policy_revision":"browser-v1","home_template_revision":"home-v1"}}}}}}"#
    )
}

fn lock_request(
    payload_version: i32,
    reason_code: Option<&str>,
    group_correlation_id: Option<&str>,
    device_id: &str,
) -> CommandRequestInput {
    CommandRequestInput {
        device_id: device_id.to_owned(),
        kind: "lock_session".to_owned(),
        payload_version,
        payload: raw(
            r#"{"target":{"session_instance_id":"session-a","session_epoch":42},"requested_lock_epoch":43}"#,
        ),
        reason_code: reason_code.map(str::to_owned),
        group_correlation_id: group_correlation_id.map(str::to_owned),
    }
}

fn assert_payload_invalid(kind: CommandKind, payload: &str) {
    match validate_payload(kind, raw(payload).as_ref()) {
        Err(CommandError::PayloadInvalid) => {}
        Err(error) => panic!("a payload violation returned the wrong typed error: {error:?}"),
        Ok(_) => panic!("an invalid payload was accepted"),
    }
}

fn assert_error<T>(result: &Result<T, CommandError>, expected: CommandError) {
    match result {
        Err(actual) => assert_eq!(*actual, expected),
        Ok(_) => panic!("an invalid request was accepted"),
    }
}

fn validated(input: CommandRequestInput) -> super::ValidatedCommandRequest {
    match validate_request(input) {
        Ok(validated) => validated,
        Err(error) => panic!("a golden request was rejected: {error:?}"),
    }
}

fn fingerprint(
    device_id: &str,
    kind: CommandKind,
    payload: &Value,
    reason_code: Option<&str>,
    group_correlation_id: Option<&str>,
) -> Vec<u8> {
    match fingerprint_v1(
        device_id,
        kind,
        1,
        payload,
        reason_code,
        group_correlation_id,
    ) {
        Ok(fingerprint) => fingerprint,
        Err(error) => panic!("a golden fingerprint could not be computed: {error:?}"),
    }
}

fn raw(value: &str) -> Box<RawValue> {
    match serde_json::from_str(value) {
        Ok(raw) => raw,
        Err(error) => {
            drop(error);
            panic!("a test payload was not valid JSON");
        }
    }
}

#[test]
fn lifecycle_transitions_are_monotonic_and_terminal_states_are_final() {
    use super::types::{CommandLifecycleState, ReportedCommandState, TransitionDecision};

    const PROGRESS: [ReportedCommandState; 2] = [
        ReportedCommandState::Received,
        ReportedCommandState::Running,
    ];
    const TERMINALS: [ReportedCommandState; 5] = [
        ReportedCommandState::Succeeded,
        ReportedCommandState::Failed,
        ReportedCommandState::Cancelled,
        ReportedCommandState::Expired,
        ReportedCommandState::ManualInterventionRequired,
    ];

    // A terminal row rejects every later report, including a different terminal: a derived
    // ordering would have ranked `failed` above `succeeded` and overwritten it.
    for current in TERMINALS {
        for reported in PROGRESS.into_iter().chain(TERMINALS) {
            let expected = if current == reported {
                TransitionDecision::DuplicateNoop
            } else {
                TransitionDecision::Regression
            };
            assert_eq!(
                CommandLifecycleState::Terminal(current).classify(reported),
                expected
            );
        }
    }

    for reported in PROGRESS.into_iter().chain(TERMINALS) {
        assert_eq!(
            CommandLifecycleState::Created.classify(reported),
            TransitionDecision::Apply
        );
    }

    assert_eq!(
        CommandLifecycleState::Received.classify(ReportedCommandState::Received),
        TransitionDecision::DuplicateNoop
    );
    assert_eq!(
        CommandLifecycleState::Received.classify(ReportedCommandState::Running),
        TransitionDecision::Apply
    );
    assert_eq!(
        CommandLifecycleState::Running.classify(ReportedCommandState::Received),
        TransitionDecision::Regression
    );
    assert_eq!(
        CommandLifecycleState::Running.classify(ReportedCommandState::Running),
        TransitionDecision::DuplicateNoop
    );
    for reported in TERMINALS {
        assert_eq!(
            CommandLifecycleState::Received.classify(reported),
            TransitionDecision::Apply
        );
        assert_eq!(
            CommandLifecycleState::Running.classify(reported),
            TransitionDecision::Apply
        );
    }

    for (text, expected) in [
        ("created", CommandLifecycleState::Created),
        ("received", CommandLifecycleState::Received),
        ("running", CommandLifecycleState::Running),
    ] {
        assert_eq!(CommandLifecycleState::parse_persisted(text), Ok(expected));
    }
    for terminal in TERMINALS {
        assert_eq!(
            CommandLifecycleState::parse_persisted(terminal.as_str()),
            Ok(CommandLifecycleState::Terminal(terminal))
        );
    }
    assert_eq!(
        CommandLifecycleState::parse_persisted("delivered"),
        Err(CommandError::PersistenceFailed)
    );
}
