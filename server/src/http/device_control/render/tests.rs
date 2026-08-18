use natsume_device_protocol::generated::{
    LockSession, OpenBindingPrompt, ResetHome, SessionTarget, SyncState, TargetAssignment,
    TargetGateway, TargetSession, TargetStateSnapshot, TerminateSession, UnlockSession, command,
    control_envelope,
};
use prost::Message as _;

use crate::application::command::{CommandKind, DispatchableCommand};

use super::{RenderError, render_wire_command};

const COMMAND_ID: &str = "01900000-0000-7000-8000-000000000103";
const LOCK_COMMAND_ID: &str = "01900000-0000-7000-8000-000000000104";
const SEAT_ID: &str = "550e8400-e29b-41d4-a716-446655440001";
const ACCOUNT_ID: &str = "550e8400-e29b-41d4-a716-446655440002";
const CREATED_AT: &str = "2026-08-16T12:34:56.789Z";
const CREATED_AT_UNIX_MS: i64 = 1_786_883_696_789;

#[test]
fn wire_render_is_byte_identical_and_lock_session_frame_matches_golden() {
    let row = row(
        CommandKind::LockSession,
        r#"{"requested_lock_epoch":43,"target":{"session_epoch":42,"session_instance_id":"session-a"}}"#,
    );
    let first = rendered(&row).encode_to_vec();
    let second = rendered(&row).encode_to_vec();
    assert_eq!(first, second);
    assert_eq!(
        hex::encode(first),
        "2a400a2430313930303030302d303030302d373030302d383030302d3030303030303030303130331095d1c5d480344a110a0d0a0973657373696f6e2d61102a102b"
    );
}

#[test]
fn sync_state_renders_every_nested_field_exactly() {
    let sync_state = body(&row(
        CommandKind::SyncState,
        &format!(
            r#"{{"canonical_hash":"{}","generation":7,"snapshot":{{"assignment":{{"account_id":"{ACCOUNT_ID}","binding_revision":11,"domjudge_username":"team-a","seat_code":"A-01","seat_id":"{SEAT_ID}"}},"gateway":{{"exact_login_policy_id":"login-v1","fixed_upstream_profile_id":"upstream-v1","gateway_certificate_min_valid_until_unix_ms":1700000000000,"gateway_certificate_profile_id":"gateway-v1","gateway_configuration_revision":12,"local_origin_hostname":"device.local"}},"schema_version":1,"session":{{"browser_policy_revision":"browser-v1","home_template_revision":"home-v1"}}}}}}"#,
            "aa".repeat(32)
        ),
    ));
    assert_eq!(
        sync_state,
        command::Body::SyncState(SyncState {
            generation: 7,
            canonical_hash: vec![0xaa; 32],
            snapshot: Some(TargetStateSnapshot {
                schema_version: 1,
                assignment: Some(TargetAssignment {
                    binding_revision: 11,
                    seat_id: SEAT_ID.to_owned(),
                    seat_code: "A-01".to_owned(),
                    account_id: ACCOUNT_ID.to_owned(),
                    domjudge_username: "team-a".to_owned(),
                }),
                gateway: Some(TargetGateway {
                    gateway_configuration_revision: 12,
                    local_origin_hostname: "device.local".to_owned(),
                    fixed_upstream_profile_id: "upstream-v1".to_owned(),
                    exact_login_policy_id: "login-v1".to_owned(),
                    gateway_certificate_profile_id: "gateway-v1".to_owned(),
                    gateway_certificate_min_valid_until_unix_ms: 1_700_000_000_000,
                }),
                session: Some(TargetSession {
                    browser_policy_revision: "browser-v1".to_owned(),
                    home_template_revision: "home-v1".to_owned(),
                }),
            }),
        })
    );
}

#[test]
fn remaining_command_kinds_render_exactly_and_sync_secret_is_held() {
    assert_eq!(
        body(&row(
            CommandKind::OpenBindingPrompt,
            r#"{"expires_at_unix_ms":1700000000000,"prompt_message_id":"prompt-1"}"#,
        )),
        command::Body::OpenBindingPrompt(OpenBindingPrompt {
            expires_at_unix_ms: 1_700_000_000_000,
            prompt_message_id: "prompt-1".to_owned(),
        })
    );
    assert_eq!(
        body(&row(
            CommandKind::LockSession,
            r#"{"requested_lock_epoch":43,"target":{"session_epoch":42,"session_instance_id":"session-a"}}"#,
        )),
        command::Body::LockSession(LockSession {
            target: Some(SessionTarget {
                session_instance_id: "session-a".to_owned(),
                session_epoch: 42,
            }),
            requested_lock_epoch: 43,
        })
    );
    assert_eq!(
        body(&row(
            CommandKind::UnlockSession,
            &format!(
                r#"{{"expected_lock_command_id":"{LOCK_COMMAND_ID}","expected_lock_epoch":43,"target":{{"session_epoch":42,"session_instance_id":"session-a"}}}}"#
            ),
        )),
        command::Body::UnlockSession(UnlockSession {
            target: Some(SessionTarget {
                session_instance_id: "session-a".to_owned(),
                session_epoch: 42,
            }),
            expected_lock_epoch: 43,
            expected_lock_command_id: LOCK_COMMAND_ID.to_owned(),
        })
    );
    assert_eq!(
        body(&row(
            CommandKind::TerminateSession,
            r#"{"target":{"session_epoch":42,"session_instance_id":"session-a"}}"#,
        )),
        command::Body::TerminateSession(TerminateSession {
            target: Some(SessionTarget {
                session_instance_id: "session-a".to_owned(),
                session_epoch: 42,
            }),
        })
    );
    assert_eq!(
        body(&row(
            CommandKind::ResetHome,
            r#"{"home_epoch":9,"home_template_revision":"home-v2"}"#,
        )),
        command::Body::ResetHome(ResetHome {
            home_template_revision: "home-v2".to_owned(),
            home_epoch: 9,
        })
    );

    let sync_secret = row(
        CommandKind::SyncSecret,
        &format!(
            r#"{{"account_id":"{ACCOUNT_ID}","binding_revision":1,"credential_revision":2,"seat_id":"{SEAT_ID}"}}"#
        ),
    );
    assert!(matches!(
        render_wire_command(&sync_secret),
        Err(RenderError::HeldByPhasePolicy)
    ));
}

#[test]
fn sqlite_strftime_timestamp_maps_to_unix_milliseconds() {
    let envelope = rendered(&row(
        CommandKind::TerminateSession,
        r#"{"target":{"session_epoch":42,"session_instance_id":"session-a"}}"#,
    ));
    let Some(control_envelope::Body::Command(command)) = envelope.body else {
        panic!("rendered envelope must contain Command");
    };
    assert_eq!(command.command_id, COMMAND_ID);
    assert_eq!(command.created_at_unix_ms, CREATED_AT_UNIX_MS);
    assert_eq!(command.deadline_unix_ms, 0);
}

fn row(kind: CommandKind, frozen_payload_json: &str) -> DispatchableCommand {
    DispatchableCommand {
        command_id: COMMAND_ID.to_owned(),
        kind,
        payload_version: 1,
        frozen_payload_json: frozen_payload_json.to_owned(),
        created_at: CREATED_AT.to_owned(),
        deadline_at: None,
    }
}

fn rendered(row: &DispatchableCommand) -> natsume_device_protocol::generated::ControlEnvelope {
    match render_wire_command(row) {
        Ok(envelope) => envelope,
        Err(error) => panic!("valid stored command did not render: {error:?}"),
    }
}

fn body(row: &DispatchableCommand) -> command::Body {
    let envelope = rendered(row);
    let Some(control_envelope::Body::Command(command)) = envelope.body else {
        panic!("rendered envelope must contain Command");
    };
    command
        .body
        .unwrap_or_else(|| panic!("rendered Command must contain a body"))
}
