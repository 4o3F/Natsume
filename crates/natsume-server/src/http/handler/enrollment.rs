use crate::http::Routes;
use crate::http::middleware::audit::RequestAudit;

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::component::device::{
    ActivationError, EnrollmentApprovalError, EnrollmentReviewId, EvidenceQuality,
    PendingEnrollmentReview,
};

use super::super::{AppState, error::ApiError, middleware};

pub(in crate::http) fn operator_routes() -> Routes {
    vec![
        ("/enrollment-reviews", get(list_enrollment_reviews)),
        (
            "/enrollment-reviews/{review_id}/actions/approve",
            middleware::require_admin(post(approve_enrollment_review)),
        ),
        (
            "/enrollment-reviews/{review_id}/actions/deny",
            middleware::require_admin(post(deny_enrollment_review)),
        ),
    ]
}

#[derive(Deserialize)]
pub(crate) struct EnrollmentReviewPath {
    /// Canonical lowercase hyphenated `UUIDv7` review ID.
    review_id: String,
}

/// Non-secret evidence awaiting an Administrator decision in this process.
#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnrollmentReviewResponse {
    review_id: String,
    machine_hardware_id: String,
    candidate_public_key: String,
    evidence_quality: EvidenceQualityResponse,
    daemon_version: String,
    agent_version: String,
}

#[derive(Serialize, specta::Type)]
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

pub(crate) async fn list_enrollment_reviews(State(state): State<AppState>) -> Response {
    let reviews = state.device().pending_enrollment_reviews().await;
    let mut response = reviews
        .iter()
        .map(EnrollmentReviewResponse::from)
        .collect::<Vec<_>>();
    response.sort_unstable_by(|left, right| left.review_id.cmp(&right.review_id));
    Json(response).into_response()
}

pub(in crate::http) async fn approve_enrollment_review(
    Extension(audit): Extension<RequestAudit>,
    State(state): State<AppState>,
    Extension(admission): Extension<crate::lifecycle::Admission>,
    Path(path): Path<EnrollmentReviewPath>,
) -> Response {
    let Some(review_id) = EnrollmentReviewId::parse(&path.review_id) else {
        return ApiError::invalid_request("enrollment_review_id_not_canonical_uuid_v7")
            .into_response();
    };
    match state
        .device_control()
        .approve_enrollment(&admission, review_id, audit.context())
        .await
    {
        Ok(authority) => {
            audit.record_target(crate::journal::event::types::AuditTarget::Device(
                authority.device_id(),
            ));
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => enrollment_approval_error(error).into_response(),
    }
}

pub(in crate::http) async fn deny_enrollment_review(
    Extension(audit): Extension<RequestAudit>,
    State(state): State<AppState>,
    Path(path): Path<EnrollmentReviewPath>,
) -> Response {
    let Some(review_id) = EnrollmentReviewId::parse(&path.review_id) else {
        return ApiError::invalid_request("enrollment_review_id_not_canonical_uuid_v7")
            .into_response();
    };
    if state
        .device()
        .deny_enrollment_review(review_id, audit.context())
        .await
    {
        StatusCode::NO_CONTENT.into_response()
    } else {
        ApiError::not_found("enrollment_review_not_found").into_response()
    }
}

fn enrollment_approval_error(error: EnrollmentApprovalError) -> ApiError {
    match error {
        EnrollmentApprovalError::ServerStopping => ApiError::unavailable("server_stopping"),
        EnrollmentApprovalError::ReviewNotFound => {
            ApiError::not_found("enrollment_review_not_found")
        }
        EnrollmentApprovalError::Authority { source: error } => ApiError::from_device(error),
        EnrollmentApprovalError::Activation { ref source } => match source.as_ref() {
            ActivationError::CandidateKeyRejected => {
                ApiError::conflict("enrollment_candidate_key_rejected")
            }
            ActivationError::InvalidAuthorityFacts { .. } => {
                ApiError::internal_error_from(error, "enrollment_invalid_authority_facts")
            }
            ActivationError::AuthorityPersistenceFailed { .. } => {
                ApiError::internal_error_from(error, "enrollment_authority_persistence_failed")
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_refusal_returns_the_mutation_gate_response() {
        let refused =
            enrollment_approval_error(EnrollmentApprovalError::ServerStopping).into_response();
        let gate = ApiError::unavailable("server_stopping").into_response();
        assert_eq!(
            refused.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        let refused = axum::body::to_bytes(refused.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("refusal body: {error}"));
        let gate = axum::body::to_bytes(gate.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("gate body: {error}"));
        assert_eq!(refused, gate);
    }
}
