use natsume_error_code::{AsErrorCode, ErrorCode};
use snafu::Snafu;

use crate::generated::{
    Command, CommandState, ControlEnvelope, DisplayBackend, GatewayCertificateMode,
    GatewayCertificateResultState, GatewayState, GraphicalSessionType, HomeState, SecretState,
    SessionAgentObservation, SessionAgentState, SessionLockState, SessionScreenKind, SessionState,
    StateApplyStatus, UiPresentationState, command, control_envelope,
};

#[derive(Debug, Snafu)]
pub enum ProtocolValidationError {
    #[snafu(display("control envelope body is missing"))]
    MissingEnvelopeBody,

    #[snafu(display("command body is missing"))]
    MissingCommandBody,

    #[snafu(display("enum field {field} is unspecified"))]
    UnspecifiedEnum { field: &'static str },

    #[snafu(display("enum field {field} contains unknown value {value}"))]
    UnknownEnum { field: &'static str, value: i32 },
}

impl AsErrorCode for ProtocolValidationError {
    fn error_code(&self) -> ErrorCode {
        ErrorCode::ProtocolInvalidEnvelope
    }
}

/// Validates one decoded envelope before it reaches command or state handlers.
///
/// # Errors
///
/// Returns [`ProtocolValidationError`] when a required oneof is empty or a validated enum
/// field is unspecified or unknown.
pub fn validate_envelope(envelope: &ControlEnvelope) -> Result<(), ProtocolValidationError> {
    let Some(body) = envelope.body.as_ref() else {
        return MissingEnvelopeBodySnafu.fail();
    };

    match body {
        control_envelope::Body::Heartbeat(heartbeat) => require_enum::<SessionLockState>(
            heartbeat.session_lock_state,
            "Heartbeat.session_lock_state",
        ),
        control_envelope::Body::ObservedState(observed) => {
            require_enum::<StateApplyStatus>(
                observed.state_apply_status,
                "ObservedStateSnapshot.state_apply_status",
            )?;
            require_enum::<SecretState>(
                observed.secret_state,
                "ObservedStateSnapshot.secret_state",
            )?;
            require_enum::<GatewayState>(
                observed.gateway_state,
                "ObservedStateSnapshot.gateway_state",
            )?;
            require_enum::<SessionState>(
                observed.session_state,
                "ObservedStateSnapshot.session_state",
            )?;
            require_enum::<SessionLockState>(
                observed.session_lock_state,
                "ObservedStateSnapshot.session_lock_state",
            )?;
            require_enum::<HomeState>(observed.home_state, "ObservedStateSnapshot.home_state")?;
            if let Some(agent) = observed.session_agent.as_ref() {
                validate_session_agent(agent)?;
            }
            Ok(())
        }
        control_envelope::Body::Command(command) => validate_command(command),
        control_envelope::Body::CommandStatus(status) => {
            require_enum::<CommandState>(status.state, "CommandStatus.state")
        }
        control_envelope::Body::BindingResult(result) => require_enum::<
            crate::generated::BindingResultState,
        >(
            result.state, "BindingResult.state"
        ),
        control_envelope::Body::GatewayCertificateResult(result) => {
            require_enum::<GatewayCertificateResultState>(
                result.state,
                "GatewayCertificateResult.state",
            )
        }
        control_envelope::Body::ClientHello(_)
        | control_envelope::Body::ServerHello(_)
        | control_envelope::Body::BindingRequest(_)
        | control_envelope::Body::GatewayCertificateRequest(_)
        | control_envelope::Body::ServerDrain(_)
        | control_envelope::Body::ProtocolError(_) => Ok(()),
    }
}

fn validate_command(value: &Command) -> Result<(), ProtocolValidationError> {
    let Some(body) = value.body.as_ref() else {
        return MissingCommandBodySnafu.fail();
    };

    if let command::Body::SyncState(sync_state) = body {
        require_enum::<GatewayCertificateMode>(
            sync_state.gateway_certificate_mode,
            "SyncState.gateway_certificate_mode",
        )?;
    }
    Ok(())
}

fn validate_session_agent(value: &SessionAgentObservation) -> Result<(), ProtocolValidationError> {
    require_enum::<SessionAgentState>(value.state, "SessionAgentObservation.state")?;
    require_enum::<GraphicalSessionType>(
        value.graphical_session_type,
        "SessionAgentObservation.graphical_session_type",
    )?;
    require_enum::<DisplayBackend>(
        value.display_backend,
        "SessionAgentObservation.display_backend",
    )?;
    require_enum::<UiPresentationState>(
        value.presentation,
        "SessionAgentObservation.presentation",
    )?;
    require_enum::<SessionScreenKind>(value.screen, "SessionAgentObservation.screen")
}

fn require_enum<T>(value: i32, field: &'static str) -> Result<(), ProtocolValidationError>
where
    T: TryFrom<i32>,
{
    if value == 0 {
        return UnspecifiedEnumSnafu { field }.fail();
    }
    if T::try_from(value).is_err() {
        return UnknownEnumSnafu { field, value }.fail();
    }
    Ok(())
}
