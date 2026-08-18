use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use diesel::{
    QueryableByName, RunQueryDsl,
    connection::SimpleConnection,
    sql_types::{BigInt, Text},
};
use natsume_device_daemon::enrollment::{EnrollmentStep, EnrollmentWaitState};
use natsume_device_protocol::CONTROL_SUBPROTOCOL;
use natsume_integration_tests::harness::{TestServer, require_ok};
use serde_json::Value;
use tokio_tungstenite::tungstenite::{Error as WebSocketError, http::header as ws_header};
use uuid::Uuid;

use super::{
    fixture::WssServerFixture as _,
    machine_id, start_server,
    wire::{close_client, connect_and_hello, expect_server_drain_and_close, open_websocket},
};

#[derive(Debug, PartialEq, Eq, QueryableByName)]
struct DeviceSecurityFacts {
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = BigInt)]
    token_count: i64,
    #[diesel(sql_type = BigInt)]
    active_certificate_count: i64,
    #[diesel(sql_type = BigInt)]
    audit_count: i64,
}

#[tokio::test]
async fn disabled_and_revoked_tokens_share_the_normalized_wss_authentication_failure() {
    let server = start_server().await;
    server.open_window().await;
    let first_hardware_id = machine_id(b"state-gated-auth-first");
    let first = server.client("state-gated-auth-first", first_hardware_id);
    first.enroll().await;
    let first_token = first.token();
    let first_device_id = server.device_id_for_hardware(first_hardware_id);

    let second_hardware_id = machine_id(b"state-gated-auth-second");
    let second = server.client("state-gated-auth-second", second_hardware_id);
    second.enroll().await;
    let second_token = second.token();

    let (mut connected_before_disable, _epoch) =
        connect_and_hello(&server, &first_token, first_hardware_id).await;
    disable_device(&server, &first_device_id).await;
    expect_server_drain_and_close(&mut connected_before_disable).await;

    let disabled_facts = device_security_facts(&server, &first_device_id);
    assert_eq!(
        disabled_facts,
        DeviceSecurityFacts {
            state: "disabled".to_owned(),
            token_count: 1,
            active_certificate_count: 1,
            audit_count: disabled_facts.audit_count,
        }
    );
    let disabled = normalized_authentication_failure(&server, Some(&first_token)).await;
    assert_eq!(
        device_security_facts(&server, &first_device_id).audit_count,
        disabled_facts.audit_count,
        "Device authentication must not create audit rows"
    );

    let (mut second_connection, _epoch) =
        connect_and_hello(&server, &second_token, second_hardware_id).await;
    close_client(&mut second_connection).await;

    server.revoke_device(&first_device_id).await;
    let revoked = normalized_authentication_failure(&server, Some(&first_token)).await;
    let missing = normalized_authentication_failure(&server, None).await;
    let malformed = normalized_authentication_failure(&server, Some("malformed")).await;
    let wrong_token = URL_SAFE_NO_PAD.encode([0xa5_u8; 32]);
    let wrong = normalized_authentication_failure(&server, Some(&wrong_token)).await;
    for normalized in [revoked, missing, malformed, wrong] {
        assert_eq!(normalized, disabled);
    }
    for _ in 0..5 {
        assert_eq!(
            normalized_authentication_failure(&server, Some(&wrong_token)).await,
            disabled
        );
    }
    assert_upgrade_rejected(
        &server,
        Some(&wrong_token),
        Some(CONTROL_SUBPROTOCOL),
        None,
        429,
        None,
    )
    .await;
    server.shutdown().await;
}

#[tokio::test]
async fn invalid_persisted_device_state_remains_an_internal_wss_failure() {
    let server = start_server().await;
    server.open_window().await;
    let hardware_id = machine_id(b"invalid-persisted-auth-state");
    let client = server.client("invalid-persisted-auth-state", hardware_id);
    client.enroll().await;
    let token = client.token();
    let mut connection = server.observer();
    require_ok(
        connection.batch_execute(
            "PRAGMA ignore_check_constraints = ON; \
             UPDATE devices SET state = 'quarantined';",
        ),
        "persisted Device state corruption must be injected",
    );

    assert_upgrade_rejected(
        &server,
        Some(&token),
        Some(CONTROL_SUBPROTOCOL),
        None,
        500,
        Some("INTERNAL_ERROR"),
    )
    .await;
    server.shutdown().await;
}

#[tokio::test]
async fn upgrade_authentication_precedes_subprotocol_and_excludes_operator_sessions() {
    let server = start_server().await;
    server.open_window().await;
    let hardware_id = machine_id(b"upgrade-auth-ordering");
    let client = server.client("upgrade-auth-ordering", hardware_id);
    client.enroll().await;
    let token = client.token();

    assert_upgrade_rejected(
        &server,
        None,
        Some(CONTROL_SUBPROTOCOL),
        Some(server.operator().cookie()),
        401,
        Some("AUTHENTICATION_FAILED"),
    )
    .await;
    assert_upgrade_rejected(
        &server,
        Some(&token),
        None,
        None,
        400,
        Some("PROTOCOL_VERSION_UNSUPPORTED"),
    )
    .await;
    assert_upgrade_rejected(
        &server,
        Some(&token),
        Some("wrong.v1"),
        None,
        400,
        Some("PROTOCOL_VERSION_UNSUPPORTED"),
    )
    .await;
    // Neither credential nor subprotocol pins authentication-before-negotiation ordering.
    assert_upgrade_rejected(
        &server,
        None,
        None,
        None,
        401,
        Some("AUTHENTICATION_FAILED"),
    )
    .await;
    server.shutdown().await;
}

/// Registration before `ClientHello` ensures revocation can evict an authenticated silent peer.
#[tokio::test]
async fn revocation_before_client_hello_still_evicts_the_authenticated_connection() {
    let server = start_server().await;
    server.open_window().await;
    let hardware_id = machine_id(b"pre-hello-revoke");
    let client = server.client("pre-hello-revoke", hardware_id);
    client.enroll().await;
    let token = client.token();

    let mut socket = open_websocket(&server, &token).await;
    let device_id = server.only_device_id().await;
    server.revoke_device(&device_id).await;
    expect_server_drain_and_close(&mut socket).await;
    server.shutdown().await;
}

#[tokio::test]
async fn replacement_claim_evicts_old_connection_and_audits_true_while_first_issue_audits_false() {
    let server = start_server().await;
    server.open_window().await;
    let hardware_id = machine_id(b"replacement-audit");
    let original = server.client("replacement-original", hardware_id);
    original.enroll().await;
    assert_eq!(server.issuance_eviction_flags(), vec![false]);
    let original_token = original.token();
    let (mut old_connection, _epoch) =
        connect_and_hello(&server, &original_token, hardware_id).await;

    let replacement = server.client("replacement-new", hardware_id);
    let pending = require_ok(
        replacement.enrollment().step().await,
        "replacement request must become pending",
    );
    server.approve_request(pending_request_id(pending)).await;
    replacement.enroll().await;

    expect_server_drain_and_close(&mut old_connection).await;
    assert_ne!(replacement.token(), original_token);
    assert_eq!(server.issuance_eviction_flags(), vec![false, true]);
    server.shutdown().await;
}

#[tokio::test]
async fn failed_authentication_rate_limit_returns_transport_429_after_ten_failures() {
    let server = start_server().await;
    let wrong_token = URL_SAFE_NO_PAD.encode([0x5a_u8; 32]);
    for _ in 0..10 {
        assert_upgrade_rejected(
            &server,
            Some(&wrong_token),
            Some(CONTROL_SUBPROTOCOL),
            None,
            401,
            Some("AUTHENTICATION_FAILED"),
        )
        .await;
    }
    assert_upgrade_rejected(
        &server,
        Some(&wrong_token),
        Some(CONTROL_SUBPROTOCOL),
        None,
        429,
        None,
    )
    .await;
    server.shutdown().await;
}

fn device_security_facts(server: &TestServer, device_id: &str) -> DeviceSecurityFacts {
    let mut connection = server.observer();
    require_ok(
        diesel::sql_query(
            "SELECT d.state AS state, \
             (SELECT COUNT(*) FROM device_tokens WHERE device_pk = d.device_pk) AS token_count, \
             (SELECT COUNT(*) FROM gateway_certificates \
              WHERE device_pk = d.device_pk AND status = 'active') AS active_certificate_count, \
             (SELECT COUNT(*) FROM audit_events) AS audit_count \
             FROM devices d WHERE d.device_pk = ?",
        )
        .bind::<Text, _>(device_id)
        .get_result(&mut connection),
        "Device security facts must be readable",
    )
}

async fn disable_device(server: &TestServer, device_id: &str) {
    let response = require_ok(
        server
            .operator_request(
                reqwest::Method::POST,
                &format!("/api/v2/devices/{device_id}/actions/disable"),
            )
            .send()
            .await,
        "Device disable must complete",
    );
    assert_eq!(response.status().as_u16(), 200);
}

async fn normalized_authentication_failure(server: &TestServer, token: Option<&str>) -> Value {
    let error = match server
        .websocket_attempt(token, Some(CONTROL_SUBPROTOCOL), None)
        .await
    {
        Err(error) => error,
        Ok((socket, _response)) => {
            drop(socket);
            panic!("WebSocket authentication must be rejected");
        }
    };
    let WebSocketError::Http(response) = error else {
        panic!("WebSocket rejection must carry the HTTP response");
    };
    assert_eq!(response.status().as_u16(), 401);
    let body = response
        .body()
        .as_deref()
        .unwrap_or_else(|| panic!("authentication failure body must be present"));
    let mut value: Value = require_ok(
        serde_json::from_slice(body),
        "authentication failure body must be JSON",
    );
    assert_eq!(
        value.get("code").and_then(Value::as_str),
        Some("AUTHENTICATION_FAILED")
    );
    let body_correlation = value
        .get("correlation_id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("authentication correlation ID must be present"));
    let header_correlation = response
        .headers()
        .get("x-correlation-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| panic!("authentication correlation header must be text"));
    assert_eq!(body_correlation, header_correlation);
    value["correlation_id"] = Value::String("normalized-correlation-id".to_owned());
    value
}

async fn assert_upgrade_rejected(
    server: &TestServer,
    token: Option<&str>,
    subprotocol: Option<&str>,
    cookie: Option<&str>,
    expected_status: u16,
    expected_code: Option<&str>,
) {
    let error = match server.websocket_attempt(token, subprotocol, cookie).await {
        Err(error) => error,
        Ok((socket, _response)) => {
            drop(socket);
            panic!("WebSocket upgrade must be rejected");
        }
    };
    let WebSocketError::Http(response) = error else {
        panic!("WebSocket rejection must carry the HTTP response");
    };
    assert_eq!(response.status().as_u16(), expected_status);
    assert!(response.headers().contains_key("x-correlation-id"));
    if let Some(expected_code) = expected_code {
        let body = response
            .body()
            .as_deref()
            .unwrap_or_else(|| panic!("stable HTTP error body must be present"));
        let value: Value = require_ok(serde_json::from_slice(body), "HTTP error body must be JSON");
        assert_eq!(
            value.get("status").and_then(Value::as_u64),
            Some(u64::from(expected_status))
        );
        assert_eq!(
            value.get("code").and_then(Value::as_str),
            Some(expected_code)
        );
        assert_eq!(
            value.get("correlation_id").and_then(Value::as_str),
            response
                .headers()
                .get("x-correlation-id")
                .and_then(|value| value.to_str().ok())
        );
    } else {
        assert!(response.body().as_deref().is_none_or(<[u8]>::is_empty));
        assert!(!response.headers().contains_key(ws_header::CONTENT_TYPE));
    }
}

fn pending_request_id(step: EnrollmentStep) -> Uuid {
    match step {
        EnrollmentStep::Waiting(EnrollmentWaitState::ApprovalPending {
            enrollment_request_id,
        }) => enrollment_request_id,
        EnrollmentStep::Enrolled
        | EnrollmentStep::Rejected
        | EnrollmentStep::Waiting(
            EnrollmentWaitState::ProvisioningWindowClosed
            | EnrollmentWaitState::NetworkUnavailable
            | EnrollmentWaitState::ServerUnavailable,
        ) => panic!("replacement Enrollment step must be approval-pending"),
    }
}
