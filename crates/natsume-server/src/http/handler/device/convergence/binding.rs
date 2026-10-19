use serde::Serialize;
use utoipa::ToSchema;

use super::ConvergenceStatusResponse;
use crate::device_control as model;

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

impl From<model::BindingConvergence> for BindingConvergenceResponse {
    fn from(value: model::BindingConvergence) -> Self {
        Self {
            status: value.status.into(),
            target: value.target.map(Into::into),
            actual: value.actual.map(Into::into),
        }
    }
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

impl From<model::BindingTarget> for BindingTargetResponse {
    fn from(value: model::BindingTarget) -> Self {
        match value {
            model::BindingTarget::Unbound {
                negotiation_id,
                evaluation,
            } => Self::Unbound {
                negotiation_id,
                evaluation: evaluation.map(Into::into),
            },
            model::BindingTarget::Bound { context } => Self::Bound {
                context: context.into(),
            },
        }
    }
}

/// Latest rejected Binding submission associated with an unbound intent.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingEvaluationResponse {
    submission_epoch: u64,
    #[schema(inline)]
    error_code: BindingEvaluationCodeResponse,
}

impl From<model::BindingEvaluation> for BindingEvaluationResponse {
    fn from(value: model::BindingEvaluation) -> Self {
        Self {
            submission_epoch: value.submission_epoch,
            error_code: value.error_code.into(),
        }
    }
}

/// Closed Binding rejection vocabulary exposed by the convergence projection.
#[derive(PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum BindingEvaluationCodeResponse {
    NotFound,
    Unmapped,
    Occupied,
}

impl From<model::BindingEvaluationCode> for BindingEvaluationCodeResponse {
    fn from(value: model::BindingEvaluationCode) -> Self {
        match value {
            model::BindingEvaluationCode::NotFound => Self::NotFound,
            model::BindingEvaluationCode::Unmapped => Self::Unmapped,
            model::BindingEvaluationCode::Occupied => Self::Occupied,
        }
    }
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

impl From<model::BindingContext> for BindingContextResponse {
    fn from(value: model::BindingContext) -> Self {
        Self {
            binding_id: value.binding_id,
            account_id: value.account_id,
            seat_code: value.seat_code,
            domjudge_username: value.domjudge_username,
            credential_revision: value.credential_revision,
        }
    }
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

impl From<model::BindingActual> for BindingActualResponse {
    fn from(value: model::BindingActual) -> Self {
        Self {
            assignment_state: value.assignment_state.into(),
            credential_state: value.credential_state.into(),
            context: value.context.map(Into::into),
        }
    }
}

/// Binding artifact state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum BindingArtifactStateResponse {
    Absent,
    Applied,
    Failed,
}

impl From<model::BindingArtifactState> for BindingArtifactStateResponse {
    fn from(value: model::BindingArtifactState) -> Self {
        match value {
            model::BindingArtifactState::Absent => Self::Absent,
            model::BindingArtifactState::Applied => Self::Applied,
            model::BindingArtifactState::Failed => Self::Failed,
        }
    }
}
