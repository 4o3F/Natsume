use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State, rejection::JsonRejection},
    http::{StatusCode, header},
    middleware as axum_middleware,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    application::{
        device::{
            HardwareIdentityQuality,
            enrollment::{
                self, EnrollmentDecisionOutcome,
                EnrollmentDecisionState as DomainEnrollmentDecisionState, EnrollmentOutcome,
                EnrollmentRequestId, EnrollmentRequestInput,
                EnrollmentRequestSummary as EnrollmentRequestSummaryFacts, EnrollmentResolution,
                EnrollmentReviewState, EnrollmentState, encode_standard_base64,
            },
        },
        operator::{self, OperatorIdentity},
    },
    audit::CorrelationId,
    tls::ClientAddress,
};

use super::super::super::{AppState, error::ApiError, middleware};

pub(crate) const ENROLLMENT_REQUEST_BODY_LIMIT_BYTES: usize = 65_536;

pub(super) fn public_routes(_state: AppState) -> Router<AppState> {
    let intake = post(create_enrollment_request)
        .layer(DefaultBodyLimit::max(ENROLLMENT_REQUEST_BODY_LIMIT_BYTES));
    Router::new().route("/enrollment-requests", intake)
}

pub(super) fn protected_routes(state: AppState) -> Router<AppState> {
    let list = middleware::operator_get(state.clone(), list_enrollment_requests);
    let approve =
        post(approve_enrollment_request).route_layer(axum_middleware::from_fn(require_admin_role));
    let reject =
        post(reject_enrollment_request).route_layer(axum_middleware::from_fn(require_admin_role));
    Router::new()
        .route("/enrollment-requests", list)
        .route(
            "/enrollment-requests/{request_id}/actions/approve",
            middleware::require_operator(state.clone(), approve),
        )
        .route(
            "/enrollment-requests/{request_id}/actions/reject",
            middleware::require_operator(state, reject),
        )
}

async fn require_admin_role(
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(identity): Extension<OperatorIdentity>,
    request: Request,
    next: Next,
) -> Response {
    if let Err(error) = operator::require_admin(identity.role()) {
        return ApiError::from_operator(error, correlation_id).into_response();
    }
    next.run(request).await
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

#[derive(Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EnrollmentHardwareIdentityQuality {
    Strong,
    Medium,
    Weak,
}

impl From<HardwareIdentityQuality> for EnrollmentHardwareIdentityQuality {
    fn from(quality: HardwareIdentityQuality) -> Self {
        match quality {
            HardwareIdentityQuality::Strong => Self::Strong,
            HardwareIdentityQuality::Medium => Self::Medium,
            HardwareIdentityQuality::Weak => Self::Weak,
        }
    }
}

impl From<EnrollmentHardwareIdentityQuality> for HardwareIdentityQuality {
    fn from(quality: EnrollmentHardwareIdentityQuality) -> Self {
        match quality {
            EnrollmentHardwareIdentityQuality::Strong => Self::Strong,
            EnrollmentHardwareIdentityQuality::Medium => Self::Medium,
            EnrollmentHardwareIdentityQuality::Weak => Self::Weak,
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
pub(crate) enum EnrollmentPendingState {
    Pending,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum EnrollmentReviewResponseState {
    Pending,
    Approved,
}

impl From<EnrollmentReviewState> for EnrollmentReviewResponseState {
    fn from(state: EnrollmentReviewState) -> Self {
        match state {
            EnrollmentReviewState::Pending => Self::Pending,
            EnrollmentReviewState::Approved => Self::Approved,
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum EnrollmentResolutionResponse {
    CreateDevice,
    ReplaceDeviceCredentials,
}

impl From<EnrollmentResolution> for EnrollmentResolutionResponse {
    fn from(resolution: EnrollmentResolution) -> Self {
        match resolution {
            EnrollmentResolution::CreateDevice => Self::CreateDevice,
            EnrollmentResolution::ReplaceDeviceCredentials => Self::ReplaceDeviceCredentials,
        }
    }
}

/// Redacted live Enrollment facts exposed to authenticated operators.
#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = EnrollmentRequestSummary)]
pub(crate) struct EnrollmentRequestSummaryResponse {
    enrollment_request_id: Uuid,
    #[schema(pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")]
    machine_hardware_id: Uuid,
    #[schema(inline)]
    hardware_identity_quality: EnrollmentHardwareIdentityQuality,
    #[schema(min_length = 64, max_length = 64, pattern = "^[0-9a-f]{64}$")]
    gateway_spki_sha256: String,
    client_version: String,
    protocol_version: u32,
    #[schema(inline)]
    state: EnrollmentReviewResponseState,
    #[schema(required = true, inline)]
    resolution: Option<EnrollmentResolutionResponse>,
    #[schema(required = true)]
    resolved_device_id: Option<Uuid>,
    /// RFC 3339 UTC timestamp with a trailing Z.
    #[schema(value_type = String, format = DateTime)]
    created_at: String,
    source_ip: String,
}

impl From<EnrollmentRequestSummaryFacts> for EnrollmentRequestSummaryResponse {
    fn from(facts: EnrollmentRequestSummaryFacts) -> Self {
        Self {
            enrollment_request_id: facts.enrollment_request_id,
            machine_hardware_id: facts.machine_hardware_id,
            hardware_identity_quality: facts.hardware_identity_quality.into(),
            gateway_spki_sha256: hex::encode(facts.gateway_spki_sha256),
            client_version: facts.client_version,
            protocol_version: facts.protocol_version,
            state: facts.state.into(),
            resolution: facts.resolution.map(EnrollmentResolutionResponse::from),
            resolved_device_id: facts.resolved_device_id.map(|device_id| device_id.value()),
            created_at: facts.created_at,
            source_ip: facts.source_ip,
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct EnrollmentActionPath {
    /// Canonical lowercase hyphenated `UUIDv7` Enrollment request ID.
    request_id: String,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnrollmentActionResponse {
    enrollment_request_id: Uuid,
    #[schema(inline)]
    state: EnrollmentActionResponseState,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum EnrollmentActionResponseState {
    Approved,
    Rejected,
}

impl From<DomainEnrollmentDecisionState> for EnrollmentActionResponseState {
    fn from(state: DomainEnrollmentDecisionState) -> Self {
        match state {
            DomainEnrollmentDecisionState::Approved => Self::Approved,
            DomainEnrollmentDecisionState::Rejected => Self::Rejected,
        }
    }
}

impl From<EnrollmentDecisionOutcome> for EnrollmentActionResponse {
    fn from(outcome: EnrollmentDecisionOutcome) -> Self {
        Self {
            enrollment_request_id: outcome.enrollment_request_id,
            state: outcome.state.into(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/enrollment-requests",
    operation_id = "listEnrollmentRequests",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Live Enrollment request set", body = [EnrollmentRequestSummaryResponse]),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn list_enrollment_requests(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
) -> Response {
    match enrollment::list_requests(&state.database).await {
        Ok(requests) => json_response(
            StatusCode::OK,
            &requests
                .into_iter()
                .map(EnrollmentRequestSummaryResponse::from)
                .collect::<Vec<_>>(),
            correlation_id,
        ),
        Err(error) => ApiError::from_enrollment(error, correlation_id).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/enrollment-requests/{request_id}/actions/approve",
    operation_id = "approveEnrollment",
    params(EnrollmentActionPath),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Enrollment request approved or already approved", body = EnrollmentActionResponse),
        (status = 400, description = "Request ID is invalid or the request is not actionable"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn approve_enrollment_request(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Path(path): Path<EnrollmentActionPath>,
) -> Response {
    apply_enrollment_decision(&state, correlation_id, path, EnrollmentDecision::Approve).await
}

#[utoipa::path(
    post,
    path = "/api/v2/enrollment-requests/{request_id}/actions/reject",
    operation_id = "rejectEnrollment",
    params(EnrollmentActionPath),
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Enrollment request rejected or already rejected", body = EnrollmentActionResponse),
        (status = 400, description = "Request ID is invalid or the request is not actionable"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn reject_enrollment_request(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Path(path): Path<EnrollmentActionPath>,
) -> Response {
    apply_enrollment_decision(&state, correlation_id, path, EnrollmentDecision::Reject).await
}

#[derive(Clone, Copy)]
enum EnrollmentDecision {
    Approve,
    Reject,
}

async fn apply_enrollment_decision(
    state: &AppState,
    correlation_id: CorrelationId,
    path: EnrollmentActionPath,
    decision: EnrollmentDecision,
) -> Response {
    let request_id = match EnrollmentRequestId::parse(&path.request_id) {
        Ok(request_id) => request_id,
        Err(error) => return ApiError::from_enrollment(error, correlation_id).into_response(),
    };
    let outcome = match decision {
        EnrollmentDecision::Approve => {
            enrollment::approve_request(&state.database, &request_id, correlation_id).await
        }
        EnrollmentDecision::Reject => {
            enrollment::reject_request(&state.database, &request_id, correlation_id).await
        }
    };
    match outcome {
        Ok(outcome) => json_response(
            StatusCode::OK,
            &EnrollmentActionResponse::from(outcome),
            correlation_id,
        ),
        Err(error) => ApiError::from_enrollment(error, correlation_id).into_response(),
    }
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
    remote_address: ClientAddress,
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
        hardware_identity_quality: request.hardware_identity_quality.into(),
        gateway_csr_der: request.gateway_csr_der,
        gateway_spki_sha256: request.gateway_spki_sha256,
        client_version: request.client_version,
        protocol_version: request.protocol_version,
    };
    match enrollment::intake_with_connection_eviction(
        &state.database,
        gateway_signer,
        input,
        source_ip,
        correlation_id,
        state.device_connections.clone(),
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
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token)
}

fn json_response<T: Serialize>(
    status: StatusCode,
    body: &T,
    correlation_id: CorrelationId,
) -> Response {
    let Ok(encoded) = serde_json::to_vec(body) else {
        return ApiError::internal_error(
            "enrollment_response_serialization_failed",
            correlation_id,
        )
        .into_response();
    };
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        encoded,
    )
        .into_response()
}

#[cfg(test)]
mod tests;
