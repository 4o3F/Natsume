use std::net::SocketAddr;

use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, State, rejection::JsonRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    application::enrollment::{
        self, EnrollmentOutcome, EnrollmentRequestInput, EnrollmentState, encode_standard_base64,
    },
    audit::CorrelationId,
};

use super::super::{AppState, error::ApiError};

pub(crate) const ENROLLMENT_REQUEST_BODY_LIMIT_BYTES: usize = 65_536;
const DEVICE_TOKEN_WIRE_LENGTH: usize = 43;
const BASE64_URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub(in crate::http) fn routes() -> Router<AppState> {
    Router::new().route(
        "/enrollment-requests",
        post(create_enrollment_request)
            .layer(DefaultBodyLimit::max(ENROLLMENT_REQUEST_BODY_LIMIT_BYTES)),
    )
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnrollmentRequest {
    #[schema(format = Uuid, pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")]
    machine_hardware_id: String,
    hardware_identity_quality: EnrollmentHardwareIdentityQuality,
    #[schema(
        value_type = String,
        format = Byte,
        min_length = 4,
        max_length = 43692,
        pattern = "^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$"
    )]
    gateway_csr_der: String,
    #[schema(min_length = 64, max_length = 64, pattern = "^[0-9a-f]{64}$")]
    gateway_spki_sha256: String,
    #[schema(min_length = 1, max_length = 64, pattern = "^[!-~]{1,64}$")]
    client_version: String,
    #[schema(minimum = 1, maximum = 1)]
    protocol_version: u32,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EnrollmentHardwareIdentityQuality {
    Strong,
    Medium,
    Weak,
}

impl EnrollmentHardwareIdentityQuality {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Medium => "medium",
            Self::Weak => "weak",
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnrollmentIssuedResponse {
    enrollment_request_id: Uuid,
    state: EnrollmentIssuedState,
    device_id: Uuid,
    #[schema(
        min_length = 43,
        max_length = 43,
        pattern = "^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$"
    )]
    device_token: String,
    #[schema(value_type = String, format = Byte)]
    gateway_leaf_der: String,
    gateway_chain_der: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EnrollmentIssuedState {
    Issued,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnrollmentPendingResponse {
    enrollment_request_id: Uuid,
    state: EnrollmentPendingState,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub(crate) enum EnrollmentPendingState {
    Pending,
    Approved,
}

#[utoipa::path(
    post,
    path = "/api/v2/enrollment-requests",
    operation_id = "createEnrollmentRequest",
    request_body = EnrollmentRequest,
    responses(
        (status = 201, description = "Device credentials issued synchronously", body = EnrollmentIssuedResponse),
        (status = 202, description = "Enrollment request awaits approval or claim", body = EnrollmentPendingResponse),
        (status = 400, description = "Invalid Enrollment request, CSR, SPKI, or protocol input"),
        (status = 409, description = "Provisioning window or Enrollment state conflict"),
        (status = 413, description = "Request body exceeds the Enrollment ingress limit"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn create_enrollment_request(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    ConnectInfo(remote_address): ConnectInfo<SocketAddr>,
    request: Result<Json<EnrollmentRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return rejection.into_response();
        }
        Err(_) => {
            return ApiError::invalid_enrollment_request(
                "enrollment_request_body_rejected",
                correlation_id,
            )
            .into_response();
        }
    };
    let Some(gateway_signer) = state.gateway_issuer.clone() else {
        return ApiError::internal_error("enrollment_issuer_unavailable", correlation_id)
            .into_response();
    };
    let source_ip = remote_address.ip();
    let input = EnrollmentRequestInput {
        machine_hardware_id: request.machine_hardware_id,
        hardware_identity_quality: request.hardware_identity_quality.as_str().to_owned(),
        gateway_csr_der: request.gateway_csr_der,
        gateway_spki_sha256: request.gateway_spki_sha256,
        client_version: request.client_version,
        protocol_version: request.protocol_version,
    };
    match enrollment::intake(
        &state.database,
        gateway_signer,
        input,
        source_ip,
        correlation_id,
    )
    .await
    {
        Ok(EnrollmentOutcome::Issued(credentials)) => json_response(
            StatusCode::CREATED,
            &EnrollmentIssuedResponse {
                enrollment_request_id: credentials.enrollment_request_id,
                state: EnrollmentIssuedState::Issued,
                device_id: credentials.device_id,
                device_token: encode_device_token(credentials.device_token.as_bytes()),
                gateway_leaf_der: encode_standard_base64(&credentials.gateway_leaf_der),
                gateway_chain_der: credentials
                    .gateway_chain_der
                    .iter()
                    .map(|certificate| encode_standard_base64(certificate))
                    .collect(),
            },
            correlation_id,
        ),
        Ok(EnrollmentOutcome::Pending(pending)) => {
            let state = match pending.state {
                EnrollmentState::Pending => EnrollmentPendingState::Pending,
            };
            json_response(
                StatusCode::ACCEPTED,
                &EnrollmentPendingResponse {
                    enrollment_request_id: pending.enrollment_request_id,
                    state,
                },
                correlation_id,
            )
        }
        Err(error) => ApiError::from_enrollment(error, correlation_id).into_response(),
    }
}

fn encode_device_token(token: &[u8; 32]) -> String {
    let mut encoded = String::with_capacity(DEVICE_TOKEN_WIRE_LENGTH);
    let mut buffer = 0_u32;
    let mut buffered_bits = 0_u32;
    for byte in token {
        buffer = (buffer << 8) | u32::from(*byte);
        buffered_bits += 8;
        while buffered_bits >= 6 {
            buffered_bits -= 6;
            encoded.push(char::from(
                BASE64_URL_ALPHABET[((buffer >> buffered_bits) & 0x3f) as usize],
            ));
        }
    }
    if buffered_bits > 0 {
        encoded.push(char::from(
            BASE64_URL_ALPHABET[((buffer << (6 - buffered_bits)) & 0x3f) as usize],
        ));
    }
    encoded
}

fn json_response<T: Serialize>(
    status: StatusCode,
    body: &T,
    correlation_id: CorrelationId,
) -> Response {
    let encoded = serde_json::to_vec(body).unwrap_or_else(|_| {
        tracing::error!(
            correlation_id = %correlation_id.as_text(),
            "Enrollment response serialization invariant failed"
        );
        panic!("Enrollment response serialization invariant failed");
    });
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        encoded,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use axum::{
        body::Body,
        extract::ConnectInfo,
        http::{Method, Request, StatusCode, header},
    };
    use diesel::{
        QueryableByName, RunQueryDsl,
        sql_types::{BigInt, Text},
    };
    use rcgen::{CertificateParams, KeyPair, PublicKeyData};
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use snafu::Snafu;
    use uuid::Uuid;

    use crate::{
        application::{
            enrollment::{
                self, GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS, GatewayIssuer,
                encode_standard_base64,
            },
            provisioning,
        },
        audit::CorrelationId,
    };

    use super::{
        super::super::{
            router_with_enrollment,
            tests::{
                SupportFailure, TestDatabase, canonical_correlation_id, check_error_response,
                drive, unused_vault_master_key, unused_web_root,
            },
        },
        ENROLLMENT_REQUEST_BODY_LIMIT_BYTES,
    };

    const REMOTE_ADDRESS: SocketAddr =
        SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(198, 51, 100, 27)), 45123);

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
        let json: Value = serde_json::from_slice(&response.body)
            .map_err(|_| TestFailure::IssuedResponseChanged)?;
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
        let pending_uuid =
            Uuid::parse_str(pending_id).map_err(|_| TestFailure::PendingFlowChanged)?;
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
            pending_uuid,
            CorrelationId::from_uuid(Uuid::now_v7()),
        )
        .await
        .map_err(|_| TestFailure::PendingFlowChanged)?;
        let rejected = drive(&application, enrollment_request(replacement_body)?).await?;
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
    async fn issuance_time_validity_margin_failure_is_stable_and_zero_write()
    -> Result<(), TestFailure> {
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

    fn valid_request_body() -> Result<String, TestFailure> {
        let key = KeyPair::generate().map_err(|_| TestFailure::RequestFailed)?;
        let params = CertificateParams::new(vec!["hostile.request.example".to_owned()])
            .map_err(|_| TestFailure::RequestFailed)?;
        let csr = params
            .serialize_request(&key)
            .map_err(|_| TestFailure::RequestFailed)?;
        let spki: [u8; 32] = Sha256::digest(key.subject_public_key_info()).into();
        Ok(serde_json::json!({
            "machine_hardware_id": Uuid::new_v5(&Uuid::NAMESPACE_OID, b"http-enrollment-machine").to_string(),
            "hardware_identity_quality": "strong",
            "gateway_csr_der": encode_standard_base64(csr.der()),
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
        request.extensions_mut().insert(ConnectInfo(REMOTE_ADDRESS));
        Ok(request)
    }

    fn canonical_uuid_v7(value: &str) -> bool {
        Uuid::parse_str(value)
            .is_ok_and(|uuid| uuid.get_version_num() == 7 && uuid.hyphenated().to_string() == value)
    }

    async fn business_count(
        fixture: &TestDatabase,
        table: &'static str,
    ) -> Result<i64, TestFailure> {
        let query = match table {
            "enrollment_requests" => "SELECT COUNT(*) AS value FROM enrollment_requests",
            _ => return Err(TestFailure::EvidenceFailed),
        };
        fixture
            .database
            .interact(move |connection| {
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
            .interact(|connection| {
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
            .interact(|connection| {
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
        #[snafu(display("Enrollment database evidence failed"))]
        EvidenceFailed,
    }
}
