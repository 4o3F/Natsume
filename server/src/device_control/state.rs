use natsume_device_protocol::generated::{
    BindingAccessTarget, BindingContext, BindingEvaluation, BindingInput as WireBindingInput,
    BindingNegotiationIntent, BoundTarget, ClientStateSnapshot, ConcreteTargetState,
    GatewayCertificateGrant, GatewayCredentialInput as WireGatewayCredentialInput,
    GatewayCredentialIntent, GatewayTarget, HomeTarget, LockState as WireLockState,
    RuntimeConfigTarget, SecretBytes, ServerIntentState, ServerStateSnapshot, SessionControlTarget,
};

use crate::{
    component::{
        binding::{
            BindingContext as ComponentBindingContext, BindingEvaluationCode, BindingInput,
            BindingNegotiationId, BindingSubmissionEpoch, MaterializedBinding,
        },
        device::DeviceId,
        gateway::{GatewayCredentialId, GatewayCredentialInput, MaterializedGateway},
        session::LockState,
    },
    server_state::ServerState,
};

use super::convergence::{self, ObservedActualState};

const SEAT_CODE_LENGTH_LIMIT: usize = 64;

/// Validates one complete Client snapshot, applies its consumed inputs, and
/// materializes one complete Server snapshot together with the validated Actual
/// retained by the current Device lease.
///
/// All wire-level Input and Actual validation finishes before the first component
/// call, so malformed or partial snapshots cause no component writes. Gateway and
/// Binding ingest run first, followed by Gateway, Binding, Runtime, Session, and Home
/// materialization. `None` covers both invalid input and component failure; the Device
/// actor gives both the same terminal lease behavior and therefore does not need a
/// richer error type.
pub(super) async fn reconcile(
    state: &ServerState,
    device_id: DeviceId,
    snapshot: ClientStateSnapshot,
) -> Option<(ServerStateSnapshot, ObservedActualState)> {
    let input = snapshot.input?;
    let actual = snapshot.actual?;
    let gateway_input = match input.gateway_credential {
        Some(input) => Some(parse_gateway_input(input)?),
        None => None,
    };
    let binding_input = match input.binding {
        Some(input) => Some(parse_binding_input(input)?),
        None => None,
    };
    let (gateway_actual, observed) = convergence::parse_actual(actual)?;

    state
        .gateway()
        .ingest(device_id, gateway_input, gateway_actual)
        .await
        .ok()?;
    state
        .binding()
        .ingest(device_id, binding_input)
        .await
        .ok()?;

    Some((materialize(state, device_id).await?, observed))
}

/// Materializes one complete Server snapshot without replaying Client input.
///
/// The Device actor uses this after an Operator target mutation has committed.
/// Component order matches initial reconciliation so every outbound message keeps
/// the same complete wire shape.
pub(super) async fn materialize(
    state: &ServerState,
    device_id: DeviceId,
) -> Option<ServerStateSnapshot> {
    let gateway = state.gateway().materialize(device_id).await.ok()?;
    let binding = state.binding().materialize(device_id).await.ok()?;
    let runtime = state.runtime().materialize().await.ok()?;
    let session = state.session().materialize(device_id).await.ok()?;
    let home = state.home().materialize(device_id).await.ok()?;

    Some(ServerStateSnapshot {
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
                lock_state: match session.lock_state() {
                    LockState::Unlocked => WireLockState::Unlocked.into(),
                    LockState::Locked => WireLockState::Locked.into(),
                },
                terminate_epoch: session.terminate_epoch(),
            }),
            home: Some(HomeTarget { reset_epoch: home }),
        }),
    })
}

/// Converts the optional Gateway Input at the wire boundary.
fn parse_gateway_input(input: WireGatewayCredentialInput) -> Option<GatewayCredentialInput> {
    let credential_id = GatewayCredentialId::parse(&input.credential_id)?;
    Some(GatewayCredentialInput::new(
        credential_id,
        input.gateway_csr_der,
    ))
}

/// Converts Binding Input only when its identifiers, epoch, and seat code are
/// canonical component inputs.
fn parse_binding_input(input: WireBindingInput) -> Option<BindingInput> {
    let negotiation_id = BindingNegotiationId::parse(&input.negotiation_id)?;
    let submission_epoch = BindingSubmissionEpoch::new(input.submission_epoch)?;
    if !valid_text(&input.seat_code, SEAT_CODE_LENGTH_LIMIT) {
        return None;
    }
    Some(BindingInput::new(
        negotiation_id,
        submission_epoch,
        input.seat_code,
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
                issuer_chain_der: certificate.issuer_chain_der().to_vec(),
            }),
    }
}

/// Encodes the already-materialized Binding target, exposing credentials only in
/// the connection-bound Server snapshot.
fn encode_binding_target(materialized: &MaterializedBinding) -> BindingAccessTarget {
    BindingAccessTarget {
        bound: materialized.target().bound().map(|bound| BoundTarget {
            context: Some(encode_binding_context(bound.context())),
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
