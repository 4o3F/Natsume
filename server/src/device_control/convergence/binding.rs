use serde::Serialize;
use utoipa::ToSchema;
use uuid::{Uuid, Variant, Version};

use natsume_device_protocol::generated::{
    BindingAccessActualState, BindingArtifactState, BindingContext as WireBindingContext,
};

use crate::component::binding::{BindingContext, BindingEvaluationCode, BindingProjection};

use super::ConvergenceStatusResponse;

const SEAT_CODE_LENGTH_LIMIT: usize = 64;
const USERNAME_LENGTH_LIMIT: usize = 128;

/// Redacted Binding target, Actual, and convergence result.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingConvergenceResponse {
    #[schema(inline)]
    pub(super) status: ConvergenceStatusResponse,
    #[schema(required = true)]
    pub(super) target: Option<BindingTargetResponse>,
    #[schema(required = true)]
    pub(super) actual: Option<BindingActualResponse>,
}

/// Current Binding intent or bound public context, excluding credentials.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(super) enum BindingTargetResponse {
    Unbound {
        #[schema(
            format = Uuid,
            pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
        )]
        negotiation_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        evaluation: Option<BindingEvaluationResponse>,
    },
    Bound {
        context: BindingContextResponse,
    },
}

/// Latest rejected Binding submission associated with an unbound intent.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingEvaluationResponse {
    submission_epoch: u64,
    #[schema(inline)]
    error_code: BindingEvaluationCodeResponse,
}

/// Closed Binding rejection vocabulary exposed by the convergence projection.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum BindingEvaluationCodeResponse {
    NotFound,
    Unmapped,
    Occupied,
}

/// Non-secret Binding identity context shared by target and Actual.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingContextResponse {
    #[schema(
        format = Uuid,
        pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
    )]
    binding_id: String,
    #[schema(
        format = Uuid,
        pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
    )]
    account_id: String,
    seat_code: String,
    domjudge_username: String,
    credential_revision: u64,
}

/// Latest validated Binding artifact Actual reported by the current lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingActualResponse {
    #[schema(inline)]
    pub(super) assignment_state: BindingArtifactStateResponse,
    #[schema(inline)]
    pub(super) credential_state: BindingArtifactStateResponse,
    #[schema(required = true)]
    pub(super) context: Option<BindingContextResponse>,
}

/// Binding artifact state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum BindingArtifactStateResponse {
    Absent,
    Applied,
    Failed,
}

pub(super) fn binding_target_response(target: BindingProjection) -> BindingTargetResponse {
    match target {
        BindingProjection::Unbound(intent) => BindingTargetResponse::Unbound {
            negotiation_id: intent.negotiation_id().as_text(),
            evaluation: intent
                .evaluation()
                .map(|evaluation| BindingEvaluationResponse {
                    submission_epoch: evaluation.submission_epoch().as_u64(),
                    error_code: match evaluation.error_code() {
                        BindingEvaluationCode::NotFound => BindingEvaluationCodeResponse::NotFound,
                        BindingEvaluationCode::Unmapped => BindingEvaluationCodeResponse::Unmapped,
                        BindingEvaluationCode::Occupied => BindingEvaluationCodeResponse::Occupied,
                    },
                }),
        },
        BindingProjection::Bound(context) => BindingTargetResponse::Bound {
            context: BindingContextResponse::from(&context),
        },
    }
}

impl From<&BindingContext> for BindingContextResponse {
    fn from(context: &BindingContext) -> Self {
        Self {
            binding_id: context.binding_id().as_text(),
            account_id: context.account_id().hyphenated().to_string(),
            seat_code: context.seat_code().to_owned(),
            domjudge_username: context.domjudge_username().to_owned(),
            credential_revision: context.credential_revision(),
        }
    }
}

pub(super) fn binding_actual_response(
    actual: BindingAccessActualState,
) -> Option<BindingActualResponse> {
    let assignment_state = binding_artifact_state(actual.assignment_state)?;
    let credential_state = binding_artifact_state(actual.credential_state)?;
    let context = match actual.context {
        Some(context) => Some(binding_context_response(context)?),
        None => None,
    };
    let requires_context = assignment_state == BindingArtifactStateResponse::Applied
        && credential_state == BindingArtifactStateResponse::Applied;
    if requires_context != context.is_some() {
        return None;
    }
    Some(BindingActualResponse {
        assignment_state,
        credential_state,
        context,
    })
}

fn binding_artifact_state(value: i32) -> Option<BindingArtifactStateResponse> {
    match BindingArtifactState::try_from(value).ok()? {
        BindingArtifactState::Unspecified => None,
        BindingArtifactState::Absent => Some(BindingArtifactStateResponse::Absent),
        BindingArtifactState::Applied => Some(BindingArtifactStateResponse::Applied),
        BindingArtifactState::Failed => Some(BindingArtifactStateResponse::Failed),
    }
}

fn binding_context_response(context: WireBindingContext) -> Option<BindingContextResponse> {
    if !is_canonical_uuid_v7(&context.binding_id)
        || !is_canonical_uuid_v7(&context.account_id)
        || !valid_text(&context.seat_code, SEAT_CODE_LENGTH_LIMIT)
        || !valid_text(&context.domjudge_username, USERNAME_LENGTH_LIMIT)
        || context.credential_revision == 0
        || context.credential_revision > i64::MAX.cast_unsigned()
    {
        return None;
    }
    Some(BindingContextResponse {
        binding_id: context.binding_id,
        account_id: context.account_id,
        seat_code: context.seat_code,
        domjudge_username: context.domjudge_username,
        credential_revision: context.credential_revision,
    })
}

pub(super) fn binding_convergence_status(
    target: Option<&BindingTargetResponse>,
    actual: Option<&BindingActualResponse>,
) -> ConvergenceStatusResponse {
    let (Some(target), Some(actual)) = (target, actual) else {
        return ConvergenceStatusResponse::AwaitingActual;
    };
    if actual.assignment_state == BindingArtifactStateResponse::Failed
        || actual.credential_state == BindingArtifactStateResponse::Failed
    {
        return ConvergenceStatusResponse::Failed;
    }
    let converged = match target {
        BindingTargetResponse::Unbound { .. } => {
            actual.assignment_state == BindingArtifactStateResponse::Absent
                && actual.credential_state == BindingArtifactStateResponse::Absent
                && actual.context.is_none()
        }
        BindingTargetResponse::Bound { context } => {
            actual.assignment_state == BindingArtifactStateResponse::Applied
                && actual.credential_state == BindingArtifactStateResponse::Applied
                && actual.context.as_ref() == Some(context)
        }
    };
    if converged {
        ConvergenceStatusResponse::Converged
    } else {
        ConvergenceStatusResponse::Drifted
    }
}

fn is_canonical_uuid_v7(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|parsed| {
        parsed.hyphenated().to_string() == value
            && parsed.get_version() == Some(Version::SortRand)
            && parsed.get_variant() == Variant::RFC4122
    })
}

fn valid_text(value: &str, length_limit: usize) -> bool {
    !value.is_empty() && value.len() <= length_limit && !value.chars().any(char::is_control)
}
