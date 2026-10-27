//! Reconcile complete Client snapshots before publishing complete Server targets.
//! Invalid Client input terminates its lease; transient persistence failures retain
//! the latest snapshot for bounded retries, while other Server failures terminate it.

use natsume_workstation_protocol::generated::{
    BindingAccessTarget, BindingContext, BindingEvaluation, BindingInput as WireBindingInput,
    BindingNegotiationIntent, BoundTarget, ClientStateSnapshot, ConcreteTargetState,
    ForegroundTarget as WireForegroundTarget, GatewayCertificateGrant,
    GatewayCredentialInput as WireGatewayCredentialInput, GatewayCredentialIntent, GatewayTarget,
    HomeTarget, PowerControlTarget, RuntimeConfigTarget, SecretBytes, ServerIntentState,
    ServerStateSnapshot, SessionControlTarget,
};
use snafu::{OptionExt as _, ResultExt as _, Snafu};

use crate::component::{
    binding::{
        BindingContext as ComponentBindingContext, BindingEvaluationCode, BindingInput,
        BindingNegotiationId, BindingSubmissionEpoch, MaterializedBinding,
    },
    device::DeviceId,
    gateway::{GatewayCredentialId, GatewayCredentialInput, MaterializedGateway},
    power::PowerControlTarget as ComponentPowerControlTarget,
    session::ForegroundTarget,
};

use super::{
    DeviceControl,
    convergence::{self, ObservedActualState},
};

const SEAT_CODE_LENGTH_LIMIT: usize = 64;

pub(super) async fn reconcile(
    control: &DeviceControl,
    device_id: DeviceId,
    snapshot: &ClientStateSnapshot,
) -> Result<(ServerStateSnapshot, ObservedActualState), ReconcileError> {
    let input = snapshot.input.as_ref().context(InvalidSnapshotSnafu)?;
    let actual = snapshot.actual.as_ref().context(InvalidSnapshotSnafu)?;
    let gateway_input = match input.gateway_credential.as_ref() {
        Some(input) => Some(parse_gateway_input(input).context(InvalidSnapshotSnafu)?),
        None => None,
    };
    let binding_input = match input.binding.as_ref() {
        Some(input) => Some(parse_binding_input(input).context(InvalidSnapshotSnafu)?),
        None => None,
    };
    let (gateway_actual, observed) =
        convergence::parse_actual(actual.clone()).context(InvalidSnapshotSnafu)?;

    control
        .gateway
        .ingest(device_id, gateway_input, gateway_actual)
        .await
        .context(GatewayIngestSnafu)?;
    control
        .binding
        .ingest(device_id, binding_input)
        .await
        .context(BindingIngestSnafu)?;

    Ok((materialize(control, device_id).await?, observed))
}

/// Materializes one complete Server snapshot without replaying Client input.
///
/// The Device actor uses this after an Operator target mutation has committed.
/// Component order matches initial reconciliation so every outbound message keeps
/// the same complete wire shape.
pub(super) async fn materialize(
    control: &DeviceControl,
    device_id: DeviceId,
) -> Result<ServerStateSnapshot, ReconcileError> {
    let gateway = control
        .gateway
        .materialize(device_id)
        .await
        .context(GatewayMaterializeSnafu)?;
    let binding = control
        .binding
        .materialize(device_id)
        .await
        .context(BindingMaterializeSnafu)?;
    let runtime = control
        .runtime
        .materialize()
        .await
        .context(RuntimeMaterializeSnafu)?;
    let session = control
        .session
        .materialize(device_id)
        .await
        .context(SessionMaterializeSnafu)?;
    let home = control
        .home
        .materialize(device_id)
        .await
        .context(HomeMaterializeSnafu)?;
    let power = control
        .power
        .materialize(device_id)
        .await
        .context(PowerMaterializeSnafu)?;

    Ok(ServerStateSnapshot {
        intent: Some(ServerIntentState {
            gateway_credential: Some(GatewayCredentialIntent {
                credential_id: gateway.intent().credential_id().as_text(),
            }),
            binding: binding.intent().map(|intent| BindingNegotiationIntent {
                negotiation_id: intent.negotiation_id().as_text(),
                evaluation: intent.evaluation().map(|evaluation| BindingEvaluation {
                    submission_epoch: evaluation.submission_epoch().as_u64(),
                    error_code: match evaluation.error_code() {
                        BindingEvaluationCode::NotFound => "SEAT_NOT_FOUND",
                        BindingEvaluationCode::Unmapped => "SEAT_UNMAPPED",
                        BindingEvaluationCode::Occupied => "SEAT_OCCUPIED",
                    }
                    .to_owned(),
                }),
            }),
        }),
        target: Some(ConcreteTargetState {
            gateway: Some(encode_gateway_target(&gateway)),
            binding_access: Some(encode_binding_target(&binding)),
            runtime_config: Some(RuntimeConfigTarget {
                domjudge_origin: runtime,
            }),
            session_control: Some(SessionControlTarget {
                foreground_target: match session.foreground_target() {
                    ForegroundTarget::Contest => WireForegroundTarget::Contest.into(),
                    ForegroundTarget::Waiting => WireForegroundTarget::Waiting.into(),
                },
                terminate_epoch: session.terminate_epoch(),
            }),
            home: Some(HomeTarget { reset_epoch: home }),
            power: Some(encode_power_target(power)),
        }),
    })
}

fn encode_power_target(target: ComponentPowerControlTarget) -> PowerControlTarget {
    PowerControlTarget {
        shutdown_epoch: target.shutdown_epoch(),
        expires_at_unix_ms: target.expires_at_unix_ms(),
    }
}

/// Converts the optional Gateway Input at the wire boundary.
fn parse_gateway_input(input: &WireGatewayCredentialInput) -> Option<GatewayCredentialInput> {
    let credential_id = GatewayCredentialId::parse(&input.credential_id)?;
    Some(GatewayCredentialInput::new(
        credential_id,
        input.gateway_csr_der.clone(),
    ))
}

/// Converts Binding Input only when its identifiers, epoch, and seat code are
/// canonical component inputs.
fn parse_binding_input(input: &WireBindingInput) -> Option<BindingInput> {
    let negotiation_id = BindingNegotiationId::parse(&input.negotiation_id)?;
    let submission_epoch = BindingSubmissionEpoch::new(input.submission_epoch)?;
    if !valid_text(&input.seat_code, SEAT_CODE_LENGTH_LIMIT) {
        return None;
    }
    Some(BindingInput::new(
        negotiation_id,
        submission_epoch,
        input.seat_code.clone(),
    ))
}

/// Encodes the already-materialized Gateway intent and target without performing
/// additional reads or mutations.
fn encode_gateway_target(materialized: &MaterializedGateway) -> GatewayTarget {
    let target = materialized.target();
    GatewayTarget {
        credential_id: target.credential_id().as_text(),
        certificate: target
            .certificate()
            .map(|certificate| GatewayCertificateGrant {
                gateway_leaf_der: certificate.leaf_der().to_vec(),
            }),
    }
}

/// Encodes the already-materialized Binding target, exposing credentials only in
/// the connection-bound Server snapshot.
fn encode_binding_target(materialized: &MaterializedBinding) -> BindingAccessTarget {
    BindingAccessTarget {
        bound: materialized.target().bound().map(|bound| BoundTarget {
            context: Some(encode_binding_context(bound.context())),
            presentation: Some(natsume_workstation_protocol::generated::TeamPresentation {
                organization_id: format!("INST-{:03}", bound.presentation().organization_id),
                team_name_zh: bound.presentation().team_name_zh.clone(),
                team_name_en: bound.presentation().team_name_en.clone(),
                school_name_zh: bound.presentation().school_name_zh.clone(),
                school_name_en: bound.presentation().school_name_en.clone(),
            }),
            password: Some(SecretBytes {
                value: bound.password().as_bytes().to_vec(),
            }),
        }),
    }
}

fn encode_binding_context(context: &ComponentBindingContext) -> BindingContext {
    BindingContext {
        binding_id: context.binding_id().as_text(),
        account_id: context.account_id().hyphenated().to_string(),
        seat_code: context.seat_code().to_owned(),
        domjudge_username: context.domjudge_username().to_owned(),
        credential_revision: context.credential_revision(),
    }
}

fn valid_text(value: &str, length_limit: usize) -> bool {
    !value.is_empty() && value.len() <= length_limit && !value.chars().any(char::is_control)
}

#[derive(Debug, Snafu)]
pub(super) enum ReconcileError {
    #[snafu(display("the Client snapshot is invalid"))]
    InvalidSnapshot,
    #[snafu(display("Device reconciliation failed at GatewayIngest"))]
    GatewayIngest {
        source: crate::component::gateway::GatewayError,
    },
    #[snafu(display("Device reconciliation failed at BindingIngest"))]
    BindingIngest {
        source: crate::component::binding::BindingError,
    },
    #[snafu(display("Device reconciliation failed at GatewayMaterialize"))]
    GatewayMaterialize {
        source: crate::component::gateway::GatewayError,
    },
    #[snafu(display("Device reconciliation failed at BindingMaterialize"))]
    BindingMaterialize {
        source: crate::component::binding::BindingError,
    },
    #[snafu(display("Device reconciliation failed at RuntimeMaterialize"))]
    RuntimeMaterialize {
        source: crate::component::runtime::RuntimeConfigError,
    },
    #[snafu(display("Device reconciliation failed at SessionMaterialize"))]
    SessionMaterialize {
        source: crate::component::session::SessionControlError,
    },
    #[snafu(display("Device reconciliation failed at HomeMaterialize"))]
    HomeMaterialize {
        source: crate::component::home::HomeError,
    },
    #[snafu(display("Device reconciliation failed at PowerMaterialize"))]
    PowerMaterialize {
        source: crate::component::power::PowerError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReconcileFault {
    Client,
    ServerTransient,
    Server,
}

impl ReconcileError {
    pub(super) fn fault(&self) -> ReconcileFault {
        use crate::component::{binding::BindingError, gateway::GatewayError};
        match self {
            Self::InvalidSnapshot
            | Self::GatewayIngest {
                source: GatewayError::InvalidCsr | GatewayError::ConflictingCsr,
            }
            | Self::BindingIngest {
                source: BindingError::ConflictingSubmission,
            } => ReconcileFault::Client,
            Self::GatewayIngest { source } | Self::GatewayMaterialize { source }
                if source.is_transient() =>
            {
                ReconcileFault::ServerTransient
            }
            Self::BindingIngest { source } | Self::BindingMaterialize { source }
                if source.is_transient() =>
            {
                ReconcileFault::ServerTransient
            }
            Self::RuntimeMaterialize { source } if source.is_transient() => {
                ReconcileFault::ServerTransient
            }
            Self::SessionMaterialize { source } if source.is_transient() => {
                ReconcileFault::ServerTransient
            }
            Self::HomeMaterialize { source } if source.is_transient() => {
                ReconcileFault::ServerTransient
            }
            Self::PowerMaterialize { source } if source.is_transient() => {
                ReconcileFault::ServerTransient
            }
            _ => ReconcileFault::Server,
        }
    }
}
