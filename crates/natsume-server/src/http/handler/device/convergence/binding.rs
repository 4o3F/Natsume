use serde::Serialize;

use super::ConvergenceStatusResponse;
use crate::device_control as model;

/// Redacted Binding target, Actual, and convergence result.
#[derive(PartialEq, Eq, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingConvergenceResponse {
    pub(super) status: ConvergenceStatusResponse,
    pub(super) target: Option<BindingTargetResponse>,
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
#[derive(PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(super) enum BindingTargetResponse {
    Unbound {
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
#[derive(PartialEq, Eq, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingEvaluationResponse {
    #[specta(type = specta_typescript::Number)]
    submission_epoch: u64,
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
#[derive(PartialEq, Eq, Serialize, specta::Type)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingContextResponse {
    binding_id: String,
    account_id: String,
    seat_code: String,
    domjudge_username: String,
    #[specta(type = specta_typescript::Number)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingActualResponse {
    pub(super) assignment_state: BindingArtifactStateResponse,
    pub(super) credential_state: BindingArtifactStateResponse,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
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
