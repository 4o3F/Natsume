use std::net::{Ipv4Addr, SocketAddr};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use diesel::{
    QueryableByName, RunQueryDsl,
    sql_types::{BigInt, Binary, Nullable, Text},
};
use rcgen::{CertificateParams, KeyPair, PublicKeyData};
use serde_json::Value;
use sha2::{Digest, Sha256};
use snafu::Snafu;
use uuid::Uuid;

use crate::application::device::enrollment::MAX_LIVE_ENROLLMENT_REQUESTS;
use crate::{
    application::{
        device::enrollment::{
            self, EnrollmentRequestId, GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS, GatewayIssuer,
            encode_standard_base64,
        },
        operator::{OperatorRole, sign_in, tests::PasswordVerificationTestGuard},
        provisioning,
    },
    audit::CorrelationId,
    db::Database,
    tls::ClientAddress,
};

use super::{
    super::super::super::{
        router_with_enrollment,
        tests::{
            Captured, SupportFailure, TestDatabase, canonical_correlation_id, check_error_response,
            drive, seed_operator, unused_vault_master_key, unused_web_root,
        },
    },
    ENROLLMENT_REQUEST_BODY_LIMIT_BYTES,
};

const REMOTE_ADDRESS: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(198, 51, 100, 27)), 45123);
const REVIEW_PASSWORD: &str = "enrollment-review-password-canary";
const LIST_EARLY_REQUEST_ID: &str = "01900000-0000-7000-8000-000000000302";
const LIST_TIE_LOW_REQUEST_ID: &str = "01900000-0000-7000-8000-000000000301";
const LIST_TIE_HIGH_REQUEST_ID: &str = "01900000-0000-7000-8000-000000000303";
const LIST_TERMINAL_REQUEST_ID: &str = "01900000-0000-7000-8000-000000000304";

#[tokio::test]
async fn route_is_role_free_correlated_window_gated_and_synchronously_issues()
-> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
    let application = router_with_enrollment(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
        gateway_signer,
    );
    let body = valid_request_body()?;
    let closed = drive(&application, enrollment_request(body.clone())?).await?;
    check_error_response(
        &closed,
        StatusCode::CONFLICT,
        "Conflict",
        "PROVISIONING_WINDOW_CLOSED",
    )?;
    if business_count(&fixture, "enrollment_requests").await? != 0 {
        return Err(TestFailure::ClosedWindowWrote);
    }

    provisioning::open_window(&fixture.database, CorrelationId::from_uuid(Uuid::now_v7()))
        .await
        .map_err(|_| TestFailure::WindowFailed)?;
    let response = drive(&application, enrollment_request(body)?).await?;
    if response.status != StatusCode::CREATED
        || canonical_correlation_id(&response.headers)?.is_empty()
        || response.headers.get(header::SET_COOKIE).is_some()
    {
        return Err(TestFailure::IssuedResponseChanged);
    }
    let json: Value =
        serde_json::from_slice(&response.body).map_err(|_| TestFailure::IssuedResponseChanged)?;
    let object = json.as_object().ok_or(TestFailure::IssuedResponseChanged)?;
    let request_id = object
        .get("enrollment_request_id")
        .and_then(Value::as_str)
        .ok_or(TestFailure::IssuedResponseChanged)?;
    let device_id = object
        .get("device_id")
        .and_then(Value::as_str)
        .ok_or(TestFailure::IssuedResponseChanged)?;
    if object.len() != 6
        || object.get("state").and_then(Value::as_str) != Some("issued")
        || object
            .get("device_token")
            .and_then(Value::as_str)
            .is_none_or(|token| token.len() != 43)
        || object
            .get("gateway_leaf_der")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || object
            .get("gateway_chain_der")
            .and_then(Value::as_array)
            .is_none_or(|chain| chain.len() != 1)
        || !canonical_uuid_v7(request_id)
        || !canonical_uuid_v7(device_id)
        || source_ip(&fixture).await? != REMOTE_ADDRESS.ip().to_string()
    {
        return Err(TestFailure::IssuedResponseChanged);
    }
    Ok(())
}

#[tokio::test]
async fn closed_json_and_body_limit_use_enrollment_mapping_before_database_work()
-> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
    let application = router_with_enrollment(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
        gateway_signer,
    );
    let malformed = serde_json::json!({
        "machine_hardware_id": "malformed-canary",
        "hardware_identity_quality": "strong",
        "gateway_csr_der": "AAAA",
        "gateway_spki_sha256": "00".repeat(32),
        "client_version": "2.0.0",
        "protocol_version": 1,
        "unexpected": "unknown-field-canary"
    })
    .to_string();
    let invalid = drive(&application, enrollment_request(malformed)?).await?;
    check_error_response(
        &invalid,
        StatusCode::BAD_REQUEST,
        "Bad Request",
        "ENROLLMENT_REQUEST_INVALID",
    )?;
    if invalid
        .body
        .windows("unknown-field-canary".len())
        .any(|window| window == "unknown-field-canary".as_bytes())
    {
        return Err(TestFailure::RejectedInputEscaped);
    }

    let oversized = "x".repeat(ENROLLMENT_REQUEST_BODY_LIMIT_BYTES + 1);
    let limited = drive(&application, enrollment_request(oversized)?).await?;
    if limited.status != StatusCode::PAYLOAD_TOO_LARGE
        || canonical_correlation_id(&limited.headers)?.is_empty()
        || business_count(&fixture, "enrollment_requests").await? != 0
    {
        return Err(TestFailure::BodyLimitChanged);
    }
    Ok(())
}

#[tokio::test]
async fn pending_replay_conflict_and_rejected_poll_have_exact_device_http_semantics()
-> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
    let application = router_with_enrollment(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
        gateway_signer,
    );
    provisioning::open_window(&fixture.database, CorrelationId::from_uuid(Uuid::now_v7()))
        .await
        .map_err(|_| TestFailure::WindowFailed)?;
    let initial = drive(&application, enrollment_request(valid_request_body()?)?).await?;
    if initial.status != StatusCode::CREATED {
        return Err(TestFailure::PendingFlowChanged);
    }

    let replacement_body = valid_request_body()?;
    let pending = drive(&application, enrollment_request(replacement_body.clone())?).await?;
    let pending_json: Value =
        serde_json::from_slice(&pending.body).map_err(|_| TestFailure::PendingFlowChanged)?;
    let pending_object = pending_json
        .as_object()
        .ok_or(TestFailure::PendingFlowChanged)?;
    let pending_id = pending_object
        .get("enrollment_request_id")
        .and_then(Value::as_str)
        .ok_or(TestFailure::PendingFlowChanged)?;
    let pending_uuid = Uuid::parse_str(pending_id).map_err(|_| TestFailure::PendingFlowChanged)?;
    if pending.status != StatusCode::ACCEPTED
        || pending_object.len() != 2
        || pending_object.get("state").and_then(Value::as_str) != Some("pending")
        || !canonical_uuid_v7(pending_id)
    {
        return Err(TestFailure::PendingFlowChanged);
    }
    let request_count = business_count(&fixture, "enrollment_requests").await?;
    let replay = drive(&application, enrollment_request(replacement_body.clone())?).await?;
    if replay.status != StatusCode::ACCEPTED
        || replay.body != pending.body
        || business_count(&fixture, "enrollment_requests").await? != request_count
    {
        return Err(TestFailure::PendingFlowChanged);
    }

    let conflict = drive(&application, enrollment_request(valid_request_body()?)?).await?;
    check_error_response(
        &conflict,
        StatusCode::CONFLICT,
        "Conflict",
        "DEVICE_IDENTITY_CONFLICT",
    )?;
    if business_count(&fixture, "enrollment_requests").await? != request_count {
        return Err(TestFailure::PendingFlowChanged);
    }

    enrollment::reject_request(
        &fixture.database,
        &EnrollmentRequestId::for_test(pending_uuid),
        CorrelationId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(|_| TestFailure::PendingFlowChanged)?;
    let rejected = drive(&application, enrollment_request(valid_request_body()?)?).await?;
    check_error_response(
        &rejected,
        StatusCode::CONFLICT,
        "Conflict",
        "ENROLLMENT_REQUEST_REJECTED",
    )?;
    if business_count(&fixture, "enrollment_requests").await? != request_count {
        return Err(TestFailure::PendingFlowChanged);
    }
    Ok(())
}

#[tokio::test]
async fn issuance_time_validity_margin_failure_is_stable_and_zero_write() -> Result<(), TestFailure>
{
    let fixture = TestDatabase::new().await?;
    provisioning::open_window(&fixture.database, CorrelationId::from_uuid(Uuid::now_v7()))
        .await
        .map_err(|_| TestFailure::WindowFailed)?;
    let before = enrollment_write_counts(&fixture).await?;
    let gateway_signer = GatewayIssuer::for_test_with_remaining_validity(
        GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS - 1,
    )
    .map_err(|_| TestFailure::IssuerFailed)?;
    let application = router_with_enrollment(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
        gateway_signer,
    );
    let response = drive(&application, enrollment_request(valid_request_body()?)?).await?;
    check_error_response(
        &response,
        StatusCode::INTERNAL_SERVER_ERROR,
        "Internal Server Error",
        "INTERNAL_ERROR",
    )?;
    if enrollment_write_counts(&fixture).await? != before {
        return Err(TestFailure::ValidityMarginFailureWrote);
    }
    Ok(())
}

#[tokio::test]
async fn forged_csr_signature_is_invalid_and_zero_write() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    provisioning::open_window(&fixture.database, CorrelationId::from_uuid(Uuid::now_v7()))
        .await
        .map_err(|_| TestFailure::WindowFailed)?;
    let before = enrollment_write_counts(&fixture).await?;
    let application = router_with_enrollment(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
        GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?,
    );
    let response = drive(
        &application,
        enrollment_request(forged_signature_request_body()?)?,
    )
    .await?;
    check_error_response(
        &response,
        StatusCode::BAD_REQUEST,
        "Bad Request",
        "ENROLLMENT_REQUEST_INVALID",
    )?;
    if enrollment_write_counts(&fixture).await? != before {
        return Err(TestFailure::ForgedCsrWrote);
    }
    Ok(())
}

#[tokio::test]
async fn live_request_capacity_is_a_stable_invalid_zero_write() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    provisioning::open_window(&fixture.database, CorrelationId::from_uuid(Uuid::now_v7()))
        .await
        .map_err(|_| TestFailure::WindowFailed)?;
    seed_live_request_capacity(&fixture).await?;
    let before = enrollment_write_counts(&fixture).await?;
    let application = router_with_enrollment(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
        GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?,
    );
    let response = drive(&application, enrollment_request(valid_request_body()?)?).await?;
    check_error_response(
        &response,
        StatusCode::BAD_REQUEST,
        "Bad Request",
        "ENROLLMENT_REQUEST_INVALID",
    )?;
    if enrollment_write_counts(&fixture).await? != before {
        return Err(TestFailure::LiveCapacityWrote);
    }
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn operator_list_is_redacted_ordered_role_shared_and_mutations_are_admin_only()
-> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let rows = review_seed_rows();
    seed_review_requests(&fixture.database, &rows).await?;
    seed_operator(
        &fixture.database,
        "enrollment-list-admin",
        OperatorRole::Admin,
        REVIEW_PASSWORD,
    )
    .await?;
    seed_operator(
        &fixture.database,
        "enrollment-list-viewer",
        OperatorRole::Viewer,
        REVIEW_PASSWORD,
    )
    .await?;
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let admin_cookie = operator_cookie(&fixture.database, "enrollment-list-admin").await?;
    let viewer_cookie = operator_cookie(&fixture.database, "enrollment-list-viewer").await?;
    let application = router_with_enrollment(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
        GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?,
    );

    let unauthenticated = drive(
        &application,
        operator_request(Method::GET, "/api/v2/enrollment-requests", None)?,
    )
    .await?;
    check_error_response(
        &unauthenticated,
        StatusCode::UNAUTHORIZED,
        "Unauthorized",
        "AUTHENTICATION_FAILED",
    )?;
    for path in [
        format!("/api/v2/enrollment-requests/{LIST_TIE_LOW_REQUEST_ID}/actions/approve"),
        format!("/api/v2/enrollment-requests/{LIST_TIE_LOW_REQUEST_ID}/actions/reject"),
    ] {
        let unauthenticated =
            drive(&application, operator_request(Method::POST, &path, None)?).await?;
        check_error_response(
            &unauthenticated,
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            "AUTHENTICATION_FAILED",
        )?;
        let viewer = drive(
            &application,
            operator_request(Method::POST, &path, Some(&viewer_cookie))?,
        )
        .await?;
        check_error_response(
            &viewer,
            StatusCode::FORBIDDEN,
            "Forbidden",
            "AUTHORIZATION_DENIED",
        )?;
    }

    let admin = drive(
        &application,
        operator_request(
            Method::GET,
            "/api/v2/enrollment-requests",
            Some(&admin_cookie),
        )?,
    )
    .await?;
    let viewer = drive(
        &application,
        operator_request(
            Method::GET,
            "/api/v2/enrollment-requests",
            Some(&viewer_cookie),
        )?,
    )
    .await?;
    if admin.status != StatusCode::OK
        || viewer.status != StatusCode::OK
        || admin.body != viewer.body
        || admin
            .body
            .windows("csr-private-canary".len())
            .any(|window| window == "csr-private-canary".as_bytes())
        || admin
            .body
            .windows("gateway_csr_der".len())
            .any(|window| window == "gateway_csr_der".as_bytes())
    {
        return Err(TestFailure::ReviewListChanged);
    }
    let listed: Value =
        serde_json::from_slice(&admin.body).map_err(|_| TestFailure::ReviewListChanged)?;
    let listed = listed.as_array().ok_or(TestFailure::ReviewListChanged)?;
    let expected_ids = [
        LIST_EARLY_REQUEST_ID,
        LIST_TIE_LOW_REQUEST_ID,
        LIST_TIE_HIGH_REQUEST_ID,
    ];
    if listed.len() != expected_ids.len() {
        return Err(TestFailure::ReviewListChanged);
    }
    for (item, expected_id) in listed.iter().zip(expected_ids) {
        let object = item.as_object().ok_or(TestFailure::ReviewListChanged)?;
        let seed = rows
            .iter()
            .find(|row| row.request_id == expected_id)
            .ok_or(TestFailure::ReviewListChanged)?;
        if object.len() != 11
            || object.get("enrollment_request_id").and_then(Value::as_str) != Some(expected_id)
            || object.get("machine_hardware_id").and_then(Value::as_str)
                != Some(seed.machine_hardware_id.as_str())
            || object
                .get("hardware_identity_quality")
                .and_then(Value::as_str)
                != Some(seed.quality)
            || object.get("gateway_spki_sha256").and_then(Value::as_str)
                != Some(hex::encode([seed.spki_byte; 32]).as_str())
            || object.get("client_version").and_then(Value::as_str) != Some(seed.client_version)
            || object.get("protocol_version").and_then(Value::as_u64) != Some(1)
            || object.get("state").and_then(Value::as_str) != Some(seed.state)
            || object.get("resolution").and_then(Value::as_str)
                != Some("replace_device_credentials")
            || object.get("resolved_device_id").and_then(Value::as_str) != Some(seed.device_id)
            || object.get("created_at").and_then(Value::as_str) != Some(seed.created_at)
            || object.get("source_ip").and_then(Value::as_str) != Some(seed.source_ip)
        {
            return Err(TestFailure::ReviewListChanged);
        }
    }
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn operator_approve_claim_and_reject_poll_flows_are_end_to_end() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    seed_operator(
        &fixture.database,
        "enrollment-flow-admin",
        OperatorRole::Admin,
        REVIEW_PASSWORD,
    )
    .await?;
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let admin_cookie = operator_cookie(&fixture.database, "enrollment-flow-admin").await?;
    provisioning::open_window(&fixture.database, CorrelationId::from_uuid(Uuid::now_v7()))
        .await
        .map_err(|_| TestFailure::WindowFailed)?;
    let application = router_with_enrollment(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
        GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?,
    );

    let approve_original = valid_request_body_for(b"approve-flow-machine")?;
    let approve_replacement = valid_request_body_for(b"approve-flow-machine")?;
    expect_status(
        &drive(&application, enrollment_request(approve_original)?).await?,
        StatusCode::CREATED,
    )?;
    let approve_pending = drive(
        &application,
        enrollment_request(approve_replacement.clone())?,
    )
    .await?;
    let approve_id = pending_response_id(&approve_pending)?;
    let credentials_before_approval = credential_row_counts(&fixture.database).await?;
    let approval = drive(
        &application,
        operator_request(
            Method::POST,
            &format!("/api/v2/enrollment-requests/{approve_id}/actions/approve"),
            Some(&admin_cookie),
        )?,
    )
    .await?;
    assert_action_response(&approval, &approve_id, "approved")?;
    if credential_row_counts(&fixture.database).await? != credentials_before_approval {
        return Err(TestFailure::ReviewApprovalIssued);
    }
    let claim = drive(&application, enrollment_request(approve_replacement)?).await?;
    expect_status(&claim, StatusCode::CREATED)?;

    let reject_original = valid_request_body_for(b"reject-flow-machine")?;
    let reject_replacement = valid_request_body_for(b"reject-flow-machine")?;
    expect_status(
        &drive(&application, enrollment_request(reject_original)?).await?,
        StatusCode::CREATED,
    )?;
    let reject_pending = drive(
        &application,
        enrollment_request(reject_replacement.clone())?,
    )
    .await?;
    let reject_id = pending_response_id(&reject_pending)?;
    let rejection = drive(
        &application,
        operator_request(
            Method::POST,
            &format!("/api/v2/enrollment-requests/{reject_id}/actions/reject"),
            Some(&admin_cookie),
        )?,
    )
    .await?;
    assert_action_response(&rejection, &reject_id, "rejected")?;
    let rejected_poll = drive(&application, enrollment_request(reject_replacement)?).await?;
    check_error_response(
        &rejected_poll,
        StatusCode::CONFLICT,
        "Conflict",
        "ENROLLMENT_REQUEST_REJECTED",
    )?;
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn decision_repeats_are_noops_and_cross_terminal_unknown_and_invalid_ids_are_closed()
-> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let rows = review_seed_rows();
    seed_review_requests(&fixture.database, &rows).await?;
    seed_operator(
        &fixture.database,
        "enrollment-decision-admin",
        OperatorRole::Admin,
        REVIEW_PASSWORD,
    )
    .await?;
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let admin_cookie = operator_cookie(&fixture.database, "enrollment-decision-admin").await?;
    let application = router_with_enrollment(
        fixture.database.clone(),
        unused_vault_master_key(),
        unused_web_root(),
        GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?,
    );

    let simple_alias_path = format!(
        "/api/v2/enrollment-requests/{}/actions/approve",
        LIST_TIE_LOW_REQUEST_ID.replace('-', "")
    );
    assert_not_actionable_without_writes(&application, &fixture, &simple_alias_path, &admin_cookie)
        .await?;

    let approve_path =
        format!("/api/v2/enrollment-requests/{LIST_TIE_LOW_REQUEST_ID}/actions/approve");
    let approval = drive(
        &application,
        operator_request(Method::POST, &approve_path, Some(&admin_cookie))?,
    )
    .await?;
    assert_action_response(&approval, LIST_TIE_LOW_REQUEST_ID, "approved")?;
    let before_approve_repeat = review_business_facts(&fixture.database).await?;
    let approval_repeat = drive(
        &application,
        operator_request(Method::POST, &approve_path, Some(&admin_cookie))?,
    )
    .await?;
    assert_action_response(&approval_repeat, LIST_TIE_LOW_REQUEST_ID, "approved")?;
    if review_business_facts(&fixture.database).await? != before_approve_repeat {
        return Err(TestFailure::ReviewNoopChangedBusinessFacts);
    }
    assert_not_actionable_without_writes(
        &application,
        &fixture,
        &format!("/api/v2/enrollment-requests/{LIST_TIE_LOW_REQUEST_ID}/actions/reject"),
        &admin_cookie,
    )
    .await?;

    let reject_path =
        format!("/api/v2/enrollment-requests/{LIST_TIE_HIGH_REQUEST_ID}/actions/reject");
    let rejection = drive(
        &application,
        operator_request(Method::POST, &reject_path, Some(&admin_cookie))?,
    )
    .await?;
    assert_action_response(&rejection, LIST_TIE_HIGH_REQUEST_ID, "rejected")?;
    let before_reject_repeat = review_business_facts(&fixture.database).await?;
    let rejection_repeat = drive(
        &application,
        operator_request(Method::POST, &reject_path, Some(&admin_cookie))?,
    )
    .await?;
    assert_action_response(&rejection_repeat, LIST_TIE_HIGH_REQUEST_ID, "rejected")?;
    if review_business_facts(&fixture.database).await? != before_reject_repeat {
        return Err(TestFailure::ReviewNoopChangedBusinessFacts);
    }
    for path in [
        format!("/api/v2/enrollment-requests/{LIST_TIE_HIGH_REQUEST_ID}/actions/approve"),
        format!("/api/v2/enrollment-requests/{LIST_TERMINAL_REQUEST_ID}/actions/approve"),
        "/api/v2/enrollment-requests/01900000-0000-7000-8000-000000000999/actions/reject"
            .to_owned(),
        "/api/v2/enrollment-requests/not-a-canonical-uuid/actions/approve".to_owned(),
    ] {
        assert_not_actionable_without_writes(&application, &fixture, &path, &admin_cookie).await?;
    }

    let already_approved = drive(
        &application,
        operator_request(
            Method::POST,
            &format!("/api/v2/enrollment-requests/{LIST_EARLY_REQUEST_ID}/actions/approve"),
            Some(&admin_cookie),
        )?,
    )
    .await?;
    assert_action_response(&already_approved, LIST_EARLY_REQUEST_ID, "approved")?;
    let audits = enrollment_decision_audits(&fixture.database).await?;
    let expected = [
        (
            LIST_TIE_LOW_REQUEST_ID,
            "approve_enrollment_request",
            "succeeded",
            "operator_requested",
        ),
        (
            LIST_TIE_LOW_REQUEST_ID,
            "approve_enrollment_request",
            "noop",
            "target_already_satisfied",
        ),
        (
            LIST_TIE_HIGH_REQUEST_ID,
            "reject_enrollment_request",
            "succeeded",
            "operator_requested",
        ),
        (
            LIST_TIE_HIGH_REQUEST_ID,
            "reject_enrollment_request",
            "noop",
            "target_already_satisfied",
        ),
        (
            LIST_EARLY_REQUEST_ID,
            "approve_enrollment_request",
            "noop",
            "target_already_satisfied",
        ),
    ];
    if audits.len() != expected.len() {
        return Err(TestFailure::ReviewAuditChanged);
    }
    for (audit, expected) in audits.iter().zip(expected) {
        if audit.actor != "operator:self"
            || audit.resource_type != "enrollment_request"
            || audit.resource_id.as_deref() != Some(expected.0)
            || audit.action_kind != expected.1
            || audit.result != expected.2
            || audit.reason_code.as_deref() != Some(expected.3)
            || audit.group_correlation_id.is_some()
            || audit.redacted_detail_json != "{}"
        {
            return Err(TestFailure::ReviewAuditChanged);
        }
    }
    Ok(())
}

fn valid_request_body() -> Result<String, TestFailure> {
    valid_request_body_for(b"http-enrollment-machine")
}

fn valid_request_body_for(machine_seed: &[u8]) -> Result<String, TestFailure> {
    let key = KeyPair::generate().map_err(|_| TestFailure::RequestFailed)?;
    let params = CertificateParams::new(vec!["hostile.request.example".to_owned()])
        .map_err(|_| TestFailure::RequestFailed)?;
    let csr = params
        .serialize_request(&key)
        .map_err(|_| TestFailure::RequestFailed)?;
    let spki: [u8; 32] = Sha256::digest(key.subject_public_key_info()).into();
    Ok(serde_json::json!({
        "machine_hardware_id": Uuid::new_v5(&Uuid::NAMESPACE_OID, machine_seed).to_string(),
        "hardware_identity_quality": "strong",
        "gateway_csr_der": encode_standard_base64(csr.der()),
        "gateway_spki_sha256": hex::encode(spki),
        "client_version": "2.0.0-test",
        "protocol_version": 1
    })
    .to_string())
}

fn forged_signature_request_body() -> Result<String, TestFailure> {
    let key = KeyPair::generate().map_err(|_| TestFailure::RequestFailed)?;
    let params = CertificateParams::new(vec!["hostile.request.example".to_owned()])
        .map_err(|_| TestFailure::RequestFailed)?;
    let mut csr_der = params
        .serialize_request(&key)
        .map_err(|_| TestFailure::RequestFailed)?
        .der()
        .to_vec();
    let signature_byte = csr_der.last_mut().ok_or(TestFailure::RequestFailed)?;
    *signature_byte ^= 0x01;
    let spki: [u8; 32] = Sha256::digest(key.subject_public_key_info()).into();
    Ok(serde_json::json!({
        "machine_hardware_id": Uuid::new_v5(&Uuid::NAMESPACE_OID, b"http-forged-csr-machine").to_string(),
        "hardware_identity_quality": "strong",
        "gateway_csr_der": encode_standard_base64(&csr_der),
        "gateway_spki_sha256": hex::encode(spki),
        "client_version": "2.0.0-test",
        "protocol_version": 1
    })
    .to_string())
}

fn enrollment_request(body: String) -> Result<Request<Body>, TestFailure> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/api/v2/enrollment-requests")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .map_err(|_| TestFailure::RequestFailed)?;
    request
        .extensions_mut()
        .insert(ConnectInfo(ClientAddress::new(REMOTE_ADDRESS)));
    Ok(request)
}

fn operator_request(
    method: Method,
    path: &str,
    cookie: Option<&str>,
) -> Result<Request<Body>, TestFailure> {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    request
        .body(Body::empty())
        .map_err(|_| TestFailure::RequestFailed)
}

async fn operator_cookie(database: &Database, login_name: &str) -> Result<String, TestFailure> {
    let session = sign_in(
        database,
        CorrelationId::from_uuid(Uuid::now_v7()),
        login_name,
        REVIEW_PASSWORD.to_owned(),
    )
    .await
    .map_err(|_| TestFailure::OperatorFixtureFailed)?;
    Ok(format!(
        "__Secure-natsume_session={}",
        session.credential().to_wire().expose()
    ))
}

fn pending_response_id(response: &Captured) -> Result<String, TestFailure> {
    if response.status != StatusCode::ACCEPTED {
        return Err(TestFailure::ReviewFlowChanged);
    }
    let value: Value =
        serde_json::from_slice(&response.body).map_err(|_| TestFailure::ReviewFlowChanged)?;
    let object = value.as_object().ok_or(TestFailure::ReviewFlowChanged)?;
    let request_id = object
        .get("enrollment_request_id")
        .and_then(Value::as_str)
        .ok_or(TestFailure::ReviewFlowChanged)?;
    if object.len() != 2
        || object.get("state").and_then(Value::as_str) != Some("pending")
        || !canonical_uuid_v7(request_id)
    {
        return Err(TestFailure::ReviewFlowChanged);
    }
    Ok(request_id.to_owned())
}

fn assert_action_response(
    response: &Captured,
    request_id: &str,
    expected_state: &str,
) -> Result<(), TestFailure> {
    if response.status != StatusCode::OK || canonical_correlation_id(&response.headers)?.is_empty()
    {
        return Err(TestFailure::ReviewActionResponseChanged);
    }
    let value: Value = serde_json::from_slice(&response.body)
        .map_err(|_| TestFailure::ReviewActionResponseChanged)?;
    let object = value
        .as_object()
        .ok_or(TestFailure::ReviewActionResponseChanged)?;
    if object.len() != 2
        || object.get("enrollment_request_id").and_then(Value::as_str) != Some(request_id)
        || object.get("state").and_then(Value::as_str) != Some(expected_state)
    {
        return Err(TestFailure::ReviewActionResponseChanged);
    }
    Ok(())
}

fn expect_status(response: &Captured, expected: StatusCode) -> Result<(), TestFailure> {
    if response.status != expected {
        return Err(TestFailure::ReviewFlowChanged);
    }
    Ok(())
}

async fn assert_not_actionable_without_writes(
    application: &Router,
    fixture: &TestDatabase,
    path: &str,
    cookie: &str,
) -> Result<(), TestFailure> {
    let before = enrollment_write_counts(fixture).await?;
    let response = drive(
        application,
        operator_request(Method::POST, path, Some(cookie))?,
    )
    .await?;
    check_error_response(
        &response,
        StatusCode::BAD_REQUEST,
        "Bad Request",
        "ENROLLMENT_REQUEST_INVALID",
    )?;
    if enrollment_write_counts(fixture).await? != before {
        return Err(TestFailure::ReviewInvalidDecisionWrote);
    }
    Ok(())
}

#[derive(Clone)]
struct ReviewSeed {
    request_id: &'static str,
    device_id: &'static str,
    machine_hardware_id: String,
    quality: &'static str,
    spki_byte: u8,
    client_version: &'static str,
    state: &'static str,
    created_at: &'static str,
    source_ip: &'static str,
}

fn review_seed_rows() -> Vec<ReviewSeed> {
    [
        (
            LIST_EARLY_REQUEST_ID,
            "01900000-0000-7000-8000-000000000402",
            "weak",
            0x22,
            "review-client-early",
            "approved",
            "2026-08-16T00:00:00.000Z",
            "192.0.2.22",
        ),
        (
            LIST_TIE_LOW_REQUEST_ID,
            "01900000-0000-7000-8000-000000000401",
            "strong",
            0x11,
            "review-client-low",
            "pending",
            "2026-08-16T00:00:01.000Z",
            "192.0.2.11",
        ),
        (
            LIST_TIE_HIGH_REQUEST_ID,
            "01900000-0000-7000-8000-000000000403",
            "medium",
            0x33,
            "review-client-high",
            "pending",
            "2026-08-16T00:00:01.000Z",
            "2001:db8::33",
        ),
        (
            LIST_TERMINAL_REQUEST_ID,
            "01900000-0000-7000-8000-000000000404",
            "strong",
            0x44,
            "review-client-terminal",
            "expired",
            "2026-08-15T23:59:59.000Z",
            "192.0.2.44",
        ),
    ]
    .into_iter()
    .map(
        |(
            request_id,
            device_id,
            quality,
            spki_byte,
            client_version,
            state,
            created_at,
            source_ip,
        )| ReviewSeed {
            request_id,
            device_id,
            machine_hardware_id: Uuid::new_v5(&Uuid::NAMESPACE_OID, request_id.as_bytes())
                .to_string(),
            quality,
            spki_byte,
            client_version,
            state,
            created_at,
            source_ip,
        },
    )
    .collect()
}

async fn seed_review_requests(database: &Database, rows: &[ReviewSeed]) -> Result<(), TestFailure> {
    let rows = rows.to_vec();
    database
        .test_write(move |connection| -> Result<(), TestFailure> {
            for row in rows {
                diesel::sql_query(
                    "INSERT INTO devices (device_pk, machine_hardware_id, \
                     hardware_identity_quality, state) VALUES (?, ?, ?, 'enrolled')",
                )
                .bind::<Text, _>(row.device_id)
                .bind::<Text, _>(&row.machine_hardware_id)
                .bind::<Text, _>(row.quality)
                .execute(connection)
                .map_err(|_| TestFailure::EvidenceFailed)?;
                let csr = format!("csr-private-canary-{}", row.request_id).into_bytes();
                let spki = [row.spki_byte; 32];
                diesel::sql_query(
                    "INSERT INTO enrollment_requests (enrollment_request_id, \
                     machine_hardware_id, hardware_identity_quality, gateway_csr_der, \
                     gateway_spki_sha256, client_version, protocol_version, source_ip, state, \
                     resolution, resolved_device_pk, issuance_audit_event_id, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, 'replace_device_credentials', ?, \
                     NULL, ?)",
                )
                .bind::<Text, _>(row.request_id)
                .bind::<Text, _>(&row.machine_hardware_id)
                .bind::<Text, _>(row.quality)
                .bind::<Binary, _>(csr)
                .bind::<Binary, _>(spki.as_slice())
                .bind::<Text, _>(row.client_version)
                .bind::<Text, _>(row.source_ip)
                .bind::<Text, _>(row.state)
                .bind::<Text, _>(row.device_id)
                .bind::<Text, _>(row.created_at)
                .execute(connection)
                .map_err(|_| TestFailure::EvidenceFailed)?;
            }
            Ok(())
        })
        .await
        .map_err(|_| TestFailure::EvidenceFailed)?
}

async fn credential_row_counts(database: &Database) -> Result<CredentialRowCounts, TestFailure> {
    database
        .test_read(|connection| {
            diesel::sql_query(
                "SELECT (SELECT COUNT(*) FROM device_tokens) AS tokens, \
                 (SELECT COUNT(*) FROM gateway_certificates) AS certificates",
            )
            .get_result(connection)
            .map_err(|_| TestFailure::EvidenceFailed)
        })
        .await
        .map_err(|_| TestFailure::EvidenceFailed)?
}

async fn review_business_facts(
    database: &Database,
) -> Result<Vec<ReviewBusinessFact>, TestFailure> {
    database
        .test_read(|connection| {
            diesel::sql_query(
                "SELECT enrollment_request_id, state FROM enrollment_requests \
                 ORDER BY enrollment_request_id",
            )
            .load(connection)
            .map_err(|_| TestFailure::EvidenceFailed)
        })
        .await
        .map_err(|_| TestFailure::EvidenceFailed)?
}

async fn enrollment_decision_audits(
    database: &Database,
) -> Result<Vec<EnrollmentDecisionAuditRow>, TestFailure> {
    database
        .test_read(|connection| {
            diesel::sql_query(
                "SELECT actor, action_kind, resource_type, resource_id, result, reason_code, \
                 group_correlation_id, redacted_detail_json FROM audit_events \
                 WHERE action_kind IN ('approve_enrollment_request', \
                 'reject_enrollment_request') ORDER BY rowid",
            )
            .load(connection)
            .map_err(|_| TestFailure::EvidenceFailed)
        })
        .await
        .map_err(|_| TestFailure::EvidenceFailed)?
}

fn canonical_uuid_v7(value: &str) -> bool {
    Uuid::parse_str(value)
        .is_ok_and(|uuid| uuid.get_version_num() == 7 && uuid.hyphenated().to_string() == value)
}

async fn business_count(fixture: &TestDatabase, table: &'static str) -> Result<i64, TestFailure> {
    let query = match table {
        "enrollment_requests" => "SELECT COUNT(*) AS value FROM enrollment_requests",
        _ => return Err(TestFailure::EvidenceFailed),
    };
    fixture
        .database
        .test_read(move |connection| {
            diesel::sql_query(query)
                .get_result::<CountRow>(connection)
                .map(|row| row.value)
                .map_err(|_| TestFailure::EvidenceFailed)
        })
        .await
        .map_err(|_| TestFailure::EvidenceFailed)?
}

async fn source_ip(fixture: &TestDatabase) -> Result<String, TestFailure> {
    fixture
        .database
        .test_read(|connection| {
            diesel::sql_query("SELECT source_ip AS value FROM enrollment_requests")
                .get_result::<StringRow>(connection)
                .map(|row| row.value)
                .map_err(|_| TestFailure::EvidenceFailed)
        })
        .await
        .map_err(|_| TestFailure::EvidenceFailed)?
}

async fn enrollment_write_counts(
    fixture: &TestDatabase,
) -> Result<EnrollmentWriteCounts, TestFailure> {
    fixture
        .database
        .test_read(|connection| {
            diesel::sql_query(
                "SELECT (SELECT COUNT(*) FROM devices) AS devices, \
                 (SELECT COUNT(*) FROM enrollment_requests) AS requests, \
                 (SELECT COUNT(*) FROM device_tokens) AS tokens, \
                 (SELECT COUNT(*) FROM gateway_certificates) AS certificates, \
                 (SELECT COUNT(*) FROM audit_events) AS audits",
            )
            .get_result(connection)
            .map_err(|_| TestFailure::EvidenceFailed)
        })
        .await
        .map_err(|_| TestFailure::EvidenceFailed)?
}

async fn seed_live_request_capacity(fixture: &TestDatabase) -> Result<(), TestFailure> {
    fixture
        .database
        .test_write(|connection| {
            diesel::sql_query(
                "WITH RECURSIVE counter(value) AS ( \
                     SELECT 1 UNION ALL SELECT value + 1 FROM counter WHERE value < ? \
                 ) INSERT INTO enrollment_requests (enrollment_request_id, \
                     machine_hardware_id, hardware_identity_quality, gateway_csr_der, \
                     gateway_spki_sha256, client_version, protocol_version, source_ip, \
                     state, resolution, resolved_device_pk, issuance_audit_event_id, created_at) \
                 SELECT printf('http-capacity-request-%03d', value), \
                     printf('http-capacity-hardware-%03d', value), 'strong', x'01', \
                     randomblob(32), 'capacity-fixture', 1, '192.0.2.201', 'pending', \
                     NULL, NULL, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM counter",
            )
            .bind::<BigInt, _>(MAX_LIVE_ENROLLMENT_REQUESTS)
            .execute(connection)
            .map(|_| ())
            .map_err(|_| TestFailure::EvidenceFailed)
        })
        .await
        .map_err(|_| TestFailure::EvidenceFailed)?
}

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

#[derive(QueryableByName)]
struct StringRow {
    #[diesel(sql_type = Text)]
    value: String,
}

#[derive(Debug, PartialEq, Eq, QueryableByName)]
struct EnrollmentWriteCounts {
    #[diesel(sql_type = BigInt)]
    devices: i64,
    #[diesel(sql_type = BigInt)]
    requests: i64,
    #[diesel(sql_type = BigInt)]
    tokens: i64,
    #[diesel(sql_type = BigInt)]
    certificates: i64,
    #[diesel(sql_type = BigInt)]
    audits: i64,
}

#[derive(Debug, PartialEq, Eq, QueryableByName)]
struct CredentialRowCounts {
    #[diesel(sql_type = BigInt)]
    tokens: i64,
    #[diesel(sql_type = BigInt)]
    certificates: i64,
}

#[derive(Debug, PartialEq, Eq, QueryableByName)]
struct ReviewBusinessFact {
    #[diesel(sql_type = Text)]
    enrollment_request_id: String,
    #[diesel(sql_type = Text)]
    state: String,
}

#[derive(QueryableByName)]
struct EnrollmentDecisionAuditRow {
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Text)]
    action_kind: String,
    #[diesel(sql_type = Text)]
    resource_type: String,
    #[diesel(sql_type = Nullable<Text>)]
    resource_id: Option<String>,
    #[diesel(sql_type = Text)]
    result: String,
    #[diesel(sql_type = Nullable<Text>)]
    reason_code: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    group_correlation_id: Option<String>,
    #[diesel(sql_type = Text)]
    redacted_detail_json: String,
}

#[derive(Debug, Snafu)]
enum TestFailure {
    #[snafu(context(false))]
    Support { source: SupportFailure },
    #[snafu(display("the Gateway issuer fixture failed"))]
    IssuerFailed,
    #[snafu(display("the Enrollment request fixture failed"))]
    RequestFailed,
    #[snafu(display("the provisioning window fixture failed"))]
    WindowFailed,
    #[snafu(display("a closed window wrote state"))]
    ClosedWindowWrote,
    #[snafu(display("the issued HTTP response changed"))]
    IssuedResponseChanged,
    #[snafu(display("rejected input escaped into the response"))]
    RejectedInputEscaped,
    #[snafu(display("the Enrollment body limit changed"))]
    BodyLimitChanged,
    #[snafu(display("the Enrollment pending HTTP flow changed"))]
    PendingFlowChanged,
    #[snafu(display("the issuance-time validity failure wrote state"))]
    ValidityMarginFailureWrote,
    #[snafu(display("a forged CSR signature wrote Enrollment state"))]
    ForgedCsrWrote,
    #[snafu(display("the live Enrollment capacity response wrote state"))]
    LiveCapacityWrote,
    #[snafu(display("the operator fixture failed"))]
    OperatorFixtureFailed,
    #[snafu(display("the Enrollment review list changed"))]
    ReviewListChanged,
    #[snafu(display("the Enrollment review flow changed"))]
    ReviewFlowChanged,
    #[snafu(display("the Enrollment review action response changed"))]
    ReviewActionResponseChanged,
    #[snafu(display("Enrollment approval issued credentials"))]
    ReviewApprovalIssued,
    #[snafu(display("an Enrollment decision noop changed business facts"))]
    ReviewNoopChangedBusinessFacts,
    #[snafu(display("an invalid Enrollment decision wrote state"))]
    ReviewInvalidDecisionWrote,
    #[snafu(display("the Enrollment decision audit changed"))]
    ReviewAuditChanged,
    #[snafu(display("Enrollment database evidence failed"))]
    EvidenceFailed,
}
