use std::time::Duration;

use diesel::connection::SimpleConnection;
use futures_util::SinkExt as _;
use natsume_device_protocol::{
    CONTROL_MAX_FRAME_BYTES, CONTROL_WIRE_VERSION,
    generated::{
        CommandState, CommandStatus, ControlEnvelope, GatewayState, Heartbeat, HomeState,
        ObservedStateSnapshot, SecretState, SessionLockState, SessionState, StateApplyStatus,
        control_envelope,
    },
};
use natsume_integration_tests::harness::{TestServer, require_ok};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message as WebSocketMessage;
use uuid::Uuid;

#[path = "wss_control/auth.rs"]
mod auth;
#[path = "wss_control/fixture.rs"]
mod fixture;
#[path = "wss_control/wire.rs"]
mod wire;

use self::{
    fixture::WssServerFixture as _,
    wire::{
        assert_command_id, assert_no_binary_frame_for, assert_ping_round_trip, client_hello,
        close_client, connect_and_hello, expect_closed, expect_no_control_envelope,
        expect_protocol_error, expect_server_drain_and_close, open_websocket, receive_envelope,
        receive_envelope_with_bytes, receive_envelope_with_timeout, send_envelope,
    },
};

const TEST_NAMESPACE: Uuid = Uuid::from_u128(0x2234_5678_1234_5678_9234_5678_1234_5678);
const OPERATOR_LOGIN: &str = "wp3-admin";
const OPERATOR_PASSWORD: &str = "wp3-operator-password";

async fn start_server() -> TestServer {
    TestServer::start(
        env!("CARGO_BIN_EXE_server-bootstrap-driver"),
        OPERATOR_LOGIN,
        OPERATOR_PASSWORD,
    )
    .await
}

#[tokio::test]
async fn hello_registry_protocol_and_frame_boundaries_are_exact() {
    let server = start_server().await;
    server.open_window().await;
    let hardware_id = machine_id(b"session-protocol");
    let client = server.client("session-protocol", hardware_id);
    client.enroll().await;
    let token = client.token();

    let (mut first, first_epoch) = connect_and_hello(&server, &token, hardware_id).await;
    let (mut second, second_epoch) = connect_and_hello(&server, &token, hardware_id).await;
    assert!(second_epoch > first_epoch);
    expect_server_drain_and_close(&mut first).await;
    assert_ping_round_trip(&mut second).await;
    close_client(&mut second).await;

    let mut not_hello = open_websocket(&server, &token).await;
    send_envelope(&mut not_hello, heartbeat_envelope()).await;
    expect_protocol_error(&mut not_hello, "PROTOCOL_INVALID_ENVELOPE").await;

    let mut wrong_version = open_websocket(&server, &token).await;
    send_envelope(&mut wrong_version, client_hello(hardware_id, 2)).await;
    expect_protocol_error(&mut wrong_version, "PROTOCOL_VERSION_UNSUPPORTED").await;

    let mut mismatch = open_websocket(&server, &token).await;
    send_envelope(
        &mut mismatch,
        client_hello(machine_id(b"different-hardware"), CONTROL_WIRE_VERSION),
    )
    .await;
    expect_protocol_error(&mut mismatch, "PROTOCOL_INVALID_ENVELOPE").await;

    let (mut observed, _epoch) = connect_and_hello(&server, &token, hardware_id).await;
    send_envelope(&mut observed, observed_envelope()).await;
    expect_protocol_error(&mut observed, "PROTOCOL_INVALID_ENVELOPE").await;

    let (mut oversized, _epoch) = connect_and_hello(&server, &token, hardware_id).await;
    require_ok(
        oversized
            .send(WebSocketMessage::Binary(
                vec![0_u8; CONTROL_MAX_FRAME_BYTES + 1].into(),
            ))
            .await,
        "oversized client frame must be written",
    );
    expect_closed(&mut oversized).await;
    server.shutdown().await;
}

#[tokio::test]
async fn durable_dispatch_wakes_on_create_converges_on_reconnect_and_redelivers_identically() {
    let server = start_server().await;
    server.open_window().await;
    let hardware_id = machine_id(b"durable-dispatch");
    let client = server.client("durable-dispatch", hardware_id);
    client.enroll().await;
    let token = client.token();
    let device_id = server.device_id_for_hardware(hardware_id);

    let (mut socket, _epoch) = connect_and_hello(&server, &token, hardware_id).await;
    let first_command_id = Uuid::now_v7();
    server
        .put_command(
            first_command_id,
            &device_id,
            "lock_session",
            lock_session_payload("dispatch-session-a", 1, 2),
        )
        .await;
    let (first_bytes, first_envelope) = receive_envelope_with_bytes(&mut socket).await;
    assert_command_id(&first_envelope, first_command_id);
    close_client(&mut socket).await;

    let (mut reconnected, _epoch) = connect_and_hello(&server, &token, hardware_id).await;
    let (redelivered_bytes, redelivered_envelope) =
        receive_envelope_with_bytes(&mut reconnected).await;
    assert_command_id(&redelivered_envelope, first_command_id);
    assert_eq!(redelivered_bytes, first_bytes);
    send_envelope(
        &mut reconnected,
        command_status_envelope(first_command_id, CommandState::Succeeded, ""),
    )
    .await;
    server
        .wait_for_command_state(first_command_id, "succeeded")
        .await;
    close_client(&mut reconnected).await;

    let offline_command_id = Uuid::now_v7();
    server
        .put_command(
            offline_command_id,
            &device_id,
            "lock_session",
            lock_session_payload("dispatch-session-b", 3, 4),
        )
        .await;
    let (mut converged, _epoch) = connect_and_hello(&server, &token, hardware_id).await;
    let offline_envelope = receive_envelope(&mut converged).await;
    assert_command_id(&offline_envelope, offline_command_id);
    send_envelope(
        &mut converged,
        command_status_envelope(offline_command_id, CommandState::Succeeded, ""),
    )
    .await;
    server
        .wait_for_command_state(offline_command_id, "succeeded")
        .await;

    let sync_secret_id = Uuid::now_v7();
    server
        .put_command(
            sync_secret_id,
            &device_id,
            "sync_secret",
            serde_json::json!({
                "seat_id": "550e8400-e29b-41d4-a716-446655440001",
                "binding_revision": 5,
                "account_id": "550e8400-e29b-41d4-a716-446655440002",
                "credential_revision": 6,
            }),
        )
        .await;
    expect_no_control_envelope(&mut converged).await;

    let following_command_id = Uuid::now_v7();
    server
        .put_command(
            following_command_id,
            &device_id,
            "lock_session",
            lock_session_payload("dispatch-session-c", 7, 8),
        )
        .await;
    let following = receive_envelope(&mut converged).await;
    assert_command_id(&following, following_command_id);
    close_client(&mut converged).await;
    server.shutdown().await;
}

#[tokio::test]
async fn heartbeat_retries_dispatch_after_the_initial_query_failure_without_another_notify() {
    let server = start_server().await;
    server.open_window().await;
    let hardware_id = machine_id(b"heartbeat-dispatch-retry");
    let client = server.client("heartbeat-dispatch-retry", hardware_id);
    client.enroll().await;
    let token = client.token();
    let device_id = server.device_id_for_hardware(hardware_id);
    let command_id = Uuid::now_v7();
    server
        .put_command(
            command_id,
            &device_id,
            "lock_session",
            lock_session_payload("heartbeat-dispatch-retry", 1, 2),
        )
        .await;

    let mut schema_connection = server.observer();
    require_ok(
        schema_connection.batch_execute("ALTER TABLE commands RENAME TO commands_wedged"),
        "command table must be temporarily unavailable",
    );
    let mut socket = open_websocket(&server, &token).await;
    send_envelope(&mut socket, client_hello(hardware_id, CONTROL_WIRE_VERSION)).await;
    let hello = receive_envelope(&mut socket).await;
    assert!(matches!(
        hello.body,
        Some(control_envelope::Body::ServerHello(_))
    ));
    // Asserting silence first both proves the wedged pass dispatched nothing and gives it time
    // to fail, so no fixed-duration sleep is needed to sequence the restore.
    expect_no_control_envelope(&mut socket).await;
    require_ok(
        schema_connection.batch_execute("ALTER TABLE commands_wedged RENAME TO commands"),
        "command table must be restored after the failed dispatch pass",
    );

    let delivered = receive_envelope_with_timeout(&mut socket, Duration::from_secs(25)).await;
    assert_command_id(&delivered, command_id);

    // A completed pass clears the retry, so the next heartbeat must not re-send the batch:
    // dispatch never changes state, and a Phase 4 device reports no terminal state at all.
    send_envelope(
        &mut socket,
        command_status_envelope(command_id, CommandState::Received, ""),
    )
    .await;
    server.wait_for_command_state(command_id, "received").await;
    assert_no_binary_frame_for(&mut socket, Duration::from_secs(25)).await;
    close_client(&mut socket).await;
    server.shutdown().await;
}

#[tokio::test]
async fn command_status_merge_ownership_audit_and_error_code_are_exact() {
    let server = start_server().await;
    server.open_window().await;
    let first_hardware_id = machine_id(b"status-first");
    let first_client = server.client("status-first", first_hardware_id);
    first_client.enroll().await;
    let first_token = first_client.token();
    let first_device_id = server.device_id_for_hardware(first_hardware_id);

    let second_hardware_id = machine_id(b"status-second");
    let second_client = server.client("status-second", second_hardware_id);
    second_client.enroll().await;
    let second_device_id = server.device_id_for_hardware(second_hardware_id);

    let command_id = Uuid::now_v7();
    server
        .put_command(
            command_id,
            &first_device_id,
            "lock_session",
            lock_session_payload("status-session", 10, 11),
        )
        .await;
    let (mut socket, _epoch) = connect_and_hello(&server, &first_token, first_hardware_id).await;
    assert_command_id(&receive_envelope(&mut socket).await, command_id);

    send_envelope(
        &mut socket,
        command_status_envelope(command_id, CommandState::Received, ""),
    )
    .await;
    server.wait_for_command_state(command_id, "received").await;
    send_envelope(
        &mut socket,
        command_status_envelope(command_id, CommandState::Running, ""),
    )
    .await;
    server.wait_for_command_state(command_id, "running").await;
    send_envelope(
        &mut socket,
        command_status_envelope(command_id, CommandState::Received, ""),
    )
    .await;
    assert_ping_round_trip(&mut socket).await;
    assert_eq!(server.command_state(command_id), "running");
    assert!(server.terminal_audits(command_id).is_empty());

    send_envelope(
        &mut socket,
        command_status_envelope(command_id, CommandState::Succeeded, ""),
    )
    .await;
    server.wait_for_command_state(command_id, "succeeded").await;
    assert_terminal_audit(&server, command_id);

    send_envelope(
        &mut socket,
        command_status_envelope(command_id, CommandState::Succeeded, ""),
    )
    .await;
    send_envelope(
        &mut socket,
        command_status_envelope(command_id, CommandState::Failed, "HOME_OPERATION_FAILED"),
    )
    .await;
    assert_ping_round_trip(&mut socket).await;
    assert_eq!(server.command_state(command_id), "succeeded");
    assert_eq!(server.terminal_audits(command_id).len(), 1);

    let foreign_command_id = Uuid::now_v7();
    server
        .put_command(
            foreign_command_id,
            &second_device_id,
            "lock_session",
            lock_session_payload("foreign-session", 1, 2),
        )
        .await;
    send_envelope(
        &mut socket,
        command_status_envelope(foreign_command_id, CommandState::Received, ""),
    )
    .await;
    assert_ping_round_trip(&mut socket).await;
    assert_eq!(server.command_state(foreign_command_id), "created");

    // An unknown command_id must be as silent as a foreign one: the device cannot learn
    // whether the row exists.
    send_envelope(
        &mut socket,
        command_status_envelope(Uuid::now_v7(), CommandState::Received, ""),
    )
    .await;
    assert_ping_round_trip(&mut socket).await;

    let (mut resumed, _epoch) = connect_and_hello(&server, &first_token, first_hardware_id).await;
    close_client(&mut resumed).await;
    server.shutdown().await;
}

/// Rejecting the frame on a command that is still `created` proves zero persistence from the
/// row itself, instead of leaning on the monotonic guard of an already-terminal command.
#[tokio::test]
async fn invalid_command_status_fields_close_the_connection_and_persist_nothing() {
    let server = start_server().await;
    server.open_window().await;
    let hardware_id = machine_id(b"invalid-command-status");
    let client = server.client("invalid-command-status", hardware_id);
    client.enroll().await;
    let token = client.token();
    let device_id = server.device_id_for_hardware(hardware_id);

    let command_id = Uuid::now_v7();
    server
        .put_command(
            command_id,
            &device_id,
            "lock_session",
            lock_session_payload("invalid-command-status", 5, 6),
        )
        .await;

    for status in [
        CommandStatus {
            command_id: command_id.to_string(),
            state: CommandState::Unspecified as i32,
            stable_error_code: String::new(),
        },
        CommandStatus {
            command_id: command_id.to_string(),
            state: i32::MAX,
            stable_error_code: String::new(),
        },
        CommandStatus {
            command_id: command_id.to_string().to_uppercase(),
            state: CommandState::Failed as i32,
            stable_error_code: String::new(),
        },
        CommandStatus {
            command_id: command_id.to_string(),
            state: CommandState::Failed as i32,
            stable_error_code: "ATTACKER_CHOSEN_CODE".to_owned(),
        },
    ] {
        let (mut socket, _epoch) = connect_and_hello(&server, &token, hardware_id).await;
        assert_command_id(&receive_envelope(&mut socket).await, command_id);
        send_envelope(
            &mut socket,
            ControlEnvelope {
                body: Some(control_envelope::Body::CommandStatus(status)),
            },
        )
        .await;
        expect_protocol_error(&mut socket, "PROTOCOL_INVALID_ENVELOPE").await;
        assert_eq!(server.command_state(command_id), "created");
        assert!(server.terminal_audits(command_id).is_empty());
    }
    server.shutdown().await;
}

fn assert_terminal_audit(server: &TestServer, command_id: Uuid) {
    let audits = server.terminal_audits(command_id);
    assert_eq!(audits.len(), 1);
    let audit = &audits[0];
    assert_eq!(audit.actor, "device:control");
    assert_eq!(audit.action_kind, "command_terminal");
    assert_eq!(audit.resource_type, "command");
    assert_eq!(
        audit.resource_id.as_deref(),
        Some(command_id.to_string().as_str())
    );
    assert_eq!(audit.result, "succeeded");
    assert_eq!(audit.reason_code.as_deref(), Some("device_reported"));
    assert!(Uuid::parse_str(&audit.correlation_id).is_ok_and(|value| {
        value.get_version_num() == 7 && value.to_string() == audit.correlation_id
    }));
    assert_eq!(
        audit.redacted_detail_json,
        r#"{"kind":"lock_session","terminal_state":"succeeded"}"#
    );
}

fn command_status_envelope(
    command_id: Uuid,
    state: CommandState,
    stable_error_code: &str,
) -> ControlEnvelope {
    ControlEnvelope {
        body: Some(control_envelope::Body::CommandStatus(CommandStatus {
            command_id: command_id.to_string(),
            state: state as i32,
            stable_error_code: stable_error_code.to_owned(),
        })),
    }
}

fn lock_session_payload(
    session_instance_id: &str,
    session_epoch: u64,
    requested_lock_epoch: u64,
) -> Value {
    serde_json::json!({
        "target": {
            "session_instance_id": session_instance_id,
            "session_epoch": session_epoch,
        },
        "requested_lock_epoch": requested_lock_epoch,
    })
}

fn heartbeat_envelope() -> ControlEnvelope {
    ControlEnvelope {
        body: Some(control_envelope::Body::Heartbeat(Heartbeat {
            session_lock_state: SessionLockState::None as i32,
            ..Heartbeat::default()
        })),
    }
}

fn observed_envelope() -> ControlEnvelope {
    ControlEnvelope {
        body: Some(control_envelope::Body::ObservedState(
            ObservedStateSnapshot {
                state_apply_status: StateApplyStatus::Idle as i32,
                secret_state: SecretState::Absent as i32,
                gateway_state: GatewayState::Absent as i32,
                session_state: SessionState::None as i32,
                session_lock_state: SessionLockState::None as i32,
                home_state: HomeState::Unmounted as i32,
                ..ObservedStateSnapshot::default()
            },
        )),
    }
}

fn machine_id(label: &[u8]) -> Uuid {
    Uuid::new_v5(&TEST_NAMESPACE, label)
}
