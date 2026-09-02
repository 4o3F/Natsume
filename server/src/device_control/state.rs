use natsume_device_protocol::generated::{
    ActualState, BindingAccessActualState, BindingAccessTarget, BindingArtifactState,
    BindingContext, BindingEvaluation, BindingInput as WireBindingInput, BindingNegotiationIntent,
    BoundTarget, ClientStateSnapshot, ConcreteTargetState,
    GatewayActualState as WireGatewayActualState, GatewayCertificateGrant,
    GatewayCredentialInput as WireGatewayCredentialInput, GatewayCredentialIntent, GatewayState,
    GatewayTarget, HomeActualState, HomeState, HomeTarget, LockState as WireLockState,
    RuntimeConfigActualState, RuntimeConfigState, RuntimeConfigTarget, SecretBytes,
    ServerIntentState, ServerStateSnapshot, SessionControlActualState, SessionControlTarget,
    SessionState,
};
use uuid::{Uuid, Variant, Version};

use crate::{
    component::{
        binding::{
            BindingContext as ComponentBindingContext, BindingEvaluationCode, BindingInput,
            BindingNegotiationId, BindingSubmissionEpoch, MaterializedBinding,
        },
        device::DeviceId,
        gateway::{
            GatewayActualState, GatewayCredentialId, GatewayCredentialInput, MaterializedGateway,
        },
        runtime::is_canonical_https_origin,
        session::LockState,
    },
    server_state::ServerState,
};

const SEAT_CODE_LENGTH_LIMIT: usize = 64;
const USERNAME_LENGTH_LIMIT: usize = 128;

/// Validates one complete Client snapshot, applies its consumed inputs, and
/// materializes one complete Server snapshot.
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
) -> Option<ServerStateSnapshot> {
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
    let gateway_actual = parse_actual(actual)?;

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

/// Requires structurally valid Actual values for all five active resources and
/// returns the Gateway Actual consumed by the current component implementation.
///
/// The other Actual values are validated but intentionally not passed to components
/// that do not consume them in WP7.
fn parse_actual(actual: ActualState) -> Option<GatewayActualState> {
    if !validate_binding_actual(actual.binding_access.as_ref()?)
        || !validate_runtime_actual(actual.runtime_config?)
        || !validate_session_actual(actual.session_control?)
        || !validate_home_actual(actual.home?)
    {
        return None;
    }
    parse_gateway_actual(actual.gateway?)
}

/// Enforces the wire-field combinations permitted for each Gateway state before
/// constructing component-owned Actual state.
fn parse_gateway_actual(actual: WireGatewayActualState) -> Option<GatewayActualState> {
    let state = GatewayState::try_from(actual.state).ok()?;
    match (state, actual.credential_id, actual.gateway_leaf_sha256) {
        (GatewayState::Absent, None, None) => Some(GatewayActualState::Absent),
        (GatewayState::Restoring, Some(credential_id), None) => {
            Some(GatewayActualState::Tracking {
                credential_id: GatewayCredentialId::parse(&credential_id)?,
            })
        }
        (GatewayState::RecoveryRequired, Some(credential_id), None) => {
            Some(GatewayActualState::RecoveryRequired {
                credential_id: GatewayCredentialId::parse(&credential_id)?,
            })
        }
        (
            GatewayState::Blocked | GatewayState::Ready | GatewayState::UpstreamUnhealthy,
            Some(credential_id),
            Some(leaf_sha256),
        ) => Some(GatewayActualState::Loaded {
            credential_id: GatewayCredentialId::parse(&credential_id)?,
            leaf_sha256: leaf_sha256.try_into().ok()?,
        }),
        _ => None,
    }
}

/// Requires Binding context exactly when both managed artifacts report `Applied`.
fn validate_binding_actual(actual: &BindingAccessActualState) -> bool {
    let Ok(assignment) = BindingArtifactState::try_from(actual.assignment_state) else {
        return false;
    };
    let Ok(credential) = BindingArtifactState::try_from(actual.credential_state) else {
        return false;
    };
    if matches!(assignment, BindingArtifactState::Unspecified)
        || matches!(credential, BindingArtifactState::Unspecified)
    {
        return false;
    }
    if assignment == BindingArtifactState::Applied && credential == BindingArtifactState::Applied {
        actual
            .context
            .as_ref()
            .is_some_and(validate_binding_context)
    } else {
        actual.context.is_none()
    }
}

fn validate_binding_context(context: &BindingContext) -> bool {
    is_canonical_uuid_v7(&context.binding_id)
        && is_canonical_uuid_v7(&context.account_id)
        && valid_text(&context.seat_code, SEAT_CODE_LENGTH_LIMIT)
        && valid_text(&context.domjudge_username, USERNAME_LENGTH_LIMIT)
        && context.credential_revision > 0
        && context.credential_revision <= i64::MAX.cast_unsigned()
}

fn validate_runtime_actual(actual: RuntimeConfigActualState) -> bool {
    let Ok(state) = RuntimeConfigState::try_from(actual.state) else {
        return false;
    };
    match (state, actual.applied_domjudge_origin) {
        (RuntimeConfigState::Absent | RuntimeConfigState::Failed, None) => true,
        (RuntimeConfigState::Applied | RuntimeConfigState::Failed, Some(origin)) => {
            is_canonical_https_origin(&origin)
        }
        _ => false,
    }
}

fn validate_session_actual(actual: SessionControlActualState) -> bool {
    SessionState::try_from(actual.session_state).is_ok_and(|state| {
        !matches!(state, SessionState::Unspecified)
            && actual
                .completed_terminate_epoch
                .is_none_or(|epoch| epoch > 0 && epoch <= i64::MAX.cast_unsigned())
    })
}

fn validate_home_actual(actual: HomeActualState) -> bool {
    HomeState::try_from(actual.state).is_ok_and(|state| {
        !matches!(state, HomeState::Unspecified)
            && actual
                .completed_reset_epoch
                .is_none_or(|epoch| epoch > 0 && epoch <= i64::MAX.cast_unsigned())
    })
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
