use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::component::device::{
    ActivationError, EnrollmentApprovalError, EnrollmentReviewId, EvidenceQuality,
    PendingEnrollmentReview,
};

use super::super::{AppState, error::ApiError, middleware};

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/enrollment-reviews",
            middleware::require_operator(state.clone(), get(list_enrollment_reviews)),
        )
        .route(
            "/enrollment-reviews/{review_id}/actions/approve",
            middleware::require_admin(state.clone(), post(approve_enrollment_review)),
        )
        .route(
            "/enrollment-reviews/{review_id}/actions/deny",
            middleware::require_admin(state, post(deny_enrollment_review)),
        )
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct EnrollmentReviewPath {
    /// Canonical lowercase hyphenated `UUIDv7` review ID.
    review_id: String,
}

/// Non-secret evidence awaiting an Administrator decision in this process.
#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnrollmentReviewResponse {
    review_id: String,
    machine_hardware_id: String,
    #[schema(
        min_length = 43,
        max_length = 43,
        pattern = "^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$"
    )]
    candidate_public_key: String,
    #[schema(inline)]
    evidence_quality: EvidenceQualityResponse,
    daemon_version: String,
    agent_version: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum EvidenceQualityResponse {
    Medium,
    Strong,
}

impl From<&PendingEnrollmentReview> for EnrollmentReviewResponse {
    fn from(review: &PendingEnrollmentReview) -> Self {
        let evidence = review.evidence();
        Self {
            review_id: review.review_id().as_text(),
            machine_hardware_id: evidence.machine_hardware_id().as_text(),
            candidate_public_key: base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(evidence.candidate_public_key().as_bytes()),
            evidence_quality: match evidence.evidence_quality() {
                EvidenceQuality::Medium => EvidenceQualityResponse::Medium,
                EvidenceQuality::Strong => EvidenceQualityResponse::Strong,
            },
            daemon_version: evidence.daemon_version().to_owned(),
            agent_version: evidence.agent_version().to_owned(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/enrollment-reviews",
    operation_id = "listEnrollmentReviews",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Enrollment reviews pending in this Server process", body = [EnrollmentReviewResponse]),
        (status = 401, description = "Session authentication failed")
    )
)]
pub(crate) async fn list_enrollment_reviews(State(state): State<AppState>) -> Response {
    let reviews = state.device().pending_enrollment_reviews().await;
    let mut response = reviews
        .iter()
        .map(EnrollmentReviewResponse::from)
        .collect::<Vec<_>>();
    response.sort_unstable_by(|left, right| left.review_id.cmp(&right.review_id));
    Json(response).into_response()
}

#[utoipa::path(
    post,
    path = "/api/v2/enrollment-reviews/{review_id}/actions/approve",
    operation_id = "approveEnrollmentReview",
    params(EnrollmentReviewPath),
    security(("sessionCookie" = [])),
    responses(
        (status = 204, description = "Enrollment authority committed and connection notification attempted"),
        (status = 400, description = "Invalid review ID"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Enrollment review unavailable"),
        (status = 409, description = "Provisioning gate closed or candidate authority rejected"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn approve_enrollment_review(
    State(state): State<AppState>,
    Path(path): Path<EnrollmentReviewPath>,
) -> Response {
    let Some(review_id) = EnrollmentReviewId::parse(&path.review_id) else {
        return ApiError::invalid_request("enrollment_review_id_not_canonical_uuid_v7")
            .into_response();
    };
    match state.device_control().approve_enrollment(review_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => enrollment_approval_error(error).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/enrollment-reviews/{review_id}/actions/deny",
    operation_id = "denyEnrollmentReview",
    params(EnrollmentReviewPath),
    security(("sessionCookie" = [])),
    responses(
        (status = 204, description = "Enrollment review terminally denied and connection notification attempted"),
        (status = 400, description = "Invalid review ID"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Enrollment review unavailable")
    )
)]
pub(crate) async fn deny_enrollment_review(
    State(state): State<AppState>,
    Path(path): Path<EnrollmentReviewPath>,
) -> Response {
    let Some(review_id) = EnrollmentReviewId::parse(&path.review_id) else {
        return ApiError::invalid_request("enrollment_review_id_not_canonical_uuid_v7")
            .into_response();
    };
    if state.device().deny_enrollment_review(review_id).await {
        StatusCode::NO_CONTENT.into_response()
    } else {
        ApiError::not_found("enrollment_review_not_found").into_response()
    }
}

fn enrollment_approval_error(error: EnrollmentApprovalError) -> ApiError {
    match error {
        EnrollmentApprovalError::ProvisioningClosed => {
            ApiError::conflict("enrollment_provisioning_closed")
        }
        EnrollmentApprovalError::ReviewNotFound => {
            ApiError::not_found("enrollment_review_not_found")
        }
        EnrollmentApprovalError::Authority(error) => ApiError::from_device(error),
        EnrollmentApprovalError::Activation(ActivationError::CandidateKeyRejected) => {
            ApiError::conflict("enrollment_candidate_key_rejected")
        }
        EnrollmentApprovalError::Activation(ActivationError::InvalidAuthorityFacts) => {
            ApiError::internal_error("enrollment_invalid_authority_facts")
        }
        EnrollmentApprovalError::Activation(ActivationError::AuthorityPersistenceFailed) => {
            ApiError::internal_error("enrollment_authority_persistence_failed")
        }
    }
}
