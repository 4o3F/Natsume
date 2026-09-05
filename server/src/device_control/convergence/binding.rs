use uuid::{Uuid, Variant, Version};

use natsume_device_protocol::generated::{
    BindingAccessActualState, BindingArtifactState as WireBindingArtifactState,
    BindingContext as WireBindingContext,
};

use crate::component::binding::{
    BindingContext as ComponentBindingContext,
    BindingEvaluationCode as ComponentBindingEvaluationCode, BindingProjection,
};

use super::ConvergenceStatus;

const SEAT_CODE_LENGTH_LIMIT: usize = 64;
const USERNAME_LENGTH_LIMIT: usize = 128;

/// Redacted Binding target, Actual, and convergence result.
#[derive(PartialEq, Eq)]
pub(crate) struct BindingConvergence {
    pub(crate) status: ConvergenceStatus,
    pub(crate) target: Option<BindingTarget>,
    pub(crate) actual: Option<BindingActual>,
}

/// Current Binding intent or bound public context, excluding credentials.
#[derive(PartialEq, Eq)]
pub(crate) enum BindingTarget {
    Unbound {
        negotiation_id: String,
        evaluation: Option<BindingEvaluation>,
    },
    Bound {
        context: BindingContext,
    },
}

/// Latest rejected Binding submission associated with an unbound intent.
#[derive(PartialEq, Eq)]
pub(crate) struct BindingEvaluation {
    pub(crate) submission_epoch: u64,
    pub(crate) error_code: BindingEvaluationCode,
}

/// Closed Binding rejection vocabulary exposed by the convergence projection.
#[derive(PartialEq, Eq)]
pub(crate) enum BindingEvaluationCode {
    NotFound,
    Unmapped,
    Occupied,
}

/// Non-secret Binding identity context shared by target and Actual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingContext {
    pub(crate) binding_id: String,
    pub(crate) account_id: String,
    pub(crate) seat_code: String,
    pub(crate) domjudge_username: String,
    pub(crate) credential_revision: u64,
}

/// Latest validated Binding artifact Actual reported by the current lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingActual {
    pub(crate) assignment_state: BindingArtifactState,
    pub(crate) credential_state: BindingArtifactState,
    pub(crate) context: Option<BindingContext>,
}

/// Binding artifact state vocabulary exposed by the convergence projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingArtifactState {
    Absent,
    Applied,
    Failed,
}

pub(super) fn binding_target(target: BindingProjection) -> BindingTarget {
    match target {
        BindingProjection::Unbound(intent) => BindingTarget::Unbound {
            negotiation_id: intent.negotiation_id().as_text(),
            evaluation: intent.evaluation().map(|evaluation| BindingEvaluation {
                submission_epoch: evaluation.submission_epoch().as_u64(),
                error_code: match evaluation.error_code() {
                    ComponentBindingEvaluationCode::NotFound => BindingEvaluationCode::NotFound,
                    ComponentBindingEvaluationCode::Unmapped => BindingEvaluationCode::Unmapped,
                    ComponentBindingEvaluationCode::Occupied => BindingEvaluationCode::Occupied,
                },
            }),
        },
        BindingProjection::Bound(context) => BindingTarget::Bound {
            context: BindingContext::from(&context),
        },
    }
}

impl From<&ComponentBindingContext> for BindingContext {
    fn from(context: &ComponentBindingContext) -> Self {
        Self {
            binding_id: context.binding_id().as_text(),
            account_id: context.account_id().hyphenated().to_string(),
            seat_code: context.seat_code().to_owned(),
            domjudge_username: context.domjudge_username().to_owned(),
            credential_revision: context.credential_revision(),
        }
    }
}

pub(super) fn parse_binding_actual(actual: BindingAccessActualState) -> Option<BindingActual> {
    let assignment_state = binding_artifact_state(actual.assignment_state)?;
    let credential_state = binding_artifact_state(actual.credential_state)?;
    let context = match actual.context {
        Some(context) => Some(parse_binding_context(context)?),
        None => None,
    };
    let requires_context = assignment_state == BindingArtifactState::Applied
        && credential_state == BindingArtifactState::Applied;
    if requires_context != context.is_some() {
        return None;
    }
    Some(BindingActual {
        assignment_state,
        credential_state,
        context,
    })
}

fn binding_artifact_state(value: i32) -> Option<BindingArtifactState> {
    match WireBindingArtifactState::try_from(value).ok()? {
        WireBindingArtifactState::Unspecified => None,
        WireBindingArtifactState::Absent => Some(BindingArtifactState::Absent),
        WireBindingArtifactState::Applied => Some(BindingArtifactState::Applied),
        WireBindingArtifactState::Failed => Some(BindingArtifactState::Failed),
    }
}

fn parse_binding_context(context: WireBindingContext) -> Option<BindingContext> {
    if !is_canonical_uuid_v7(&context.binding_id)
        || !is_canonical_uuid_v7(&context.account_id)
        || !valid_text(&context.seat_code, SEAT_CODE_LENGTH_LIMIT)
        || !valid_text(&context.domjudge_username, USERNAME_LENGTH_LIMIT)
        || context.credential_revision == 0
        || context.credential_revision > i64::MAX.cast_unsigned()
    {
        return None;
    }
    Some(BindingContext {
        binding_id: context.binding_id,
        account_id: context.account_id,
        seat_code: context.seat_code,
        domjudge_username: context.domjudge_username,
        credential_revision: context.credential_revision,
    })
}

pub(super) fn binding_convergence_status(
    target: Option<&BindingTarget>,
    actual: Option<&BindingActual>,
) -> ConvergenceStatus {
    let (Some(target), Some(actual)) = (target, actual) else {
        return ConvergenceStatus::AwaitingActual;
    };
    if actual.assignment_state == BindingArtifactState::Failed
        || actual.credential_state == BindingArtifactState::Failed
    {
        return ConvergenceStatus::Failed;
    }
    let converged = match target {
        BindingTarget::Unbound { .. } => {
            actual.assignment_state == BindingArtifactState::Absent
                && actual.credential_state == BindingArtifactState::Absent
                && actual.context.is_none()
        }
        BindingTarget::Bound { context } => {
            actual.assignment_state == BindingArtifactState::Applied
                && actual.credential_state == BindingArtifactState::Applied
                && actual.context.as_ref() == Some(context)
        }
    };
    if converged {
        ConvergenceStatus::Converged
    } else {
        ConvergenceStatus::Drifted
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
