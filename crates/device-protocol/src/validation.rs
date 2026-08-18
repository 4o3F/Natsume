use natsume_error_code::{ErrorCode, control::ControlErrorCode};
use snafu::Snafu;
use uuid::{Uuid, Variant, Version};

use crate::generated::{
    Command, CommandState, ControlEnvelope, DisplayBackend, GatewayState, GraphicalSessionType,
    HomeState, SecretState, SessionAgentObservation, SessionAgentState, SessionLockState,
    SessionScreenKind, SessionState, StateApplyStatus, UiPresentationState, control_envelope,
};

#[derive(Debug, Snafu)]
pub enum ProtocolValidationError {
    #[snafu(display("control envelope body is missing"))]
    MissingEnvelopeBody,

    #[snafu(display("command body is missing"))]
    MissingCommandBody,

    #[snafu(display("command ID must be a canonical lowercase UUIDv7"))]
    InvalidCommandId,

    #[snafu(display("enum field {field} is unspecified"))]
    UnspecifiedEnum { field: &'static str },

    #[snafu(display("enum field {field} contains unknown value {value}"))]
    UnknownEnum { field: &'static str, value: i32 },
}

impl ProtocolValidationError {
    /// Maps a typed validation failure to its stable public control semantic.
    #[must_use]
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            Self::InvalidCommandId => ErrorCode::Control(ControlErrorCode::CommandIdInvalid),
            Self::MissingEnvelopeBody
            | Self::MissingCommandBody
            | Self::UnspecifiedEnum { .. }
            | Self::UnknownEnum { .. } => {
                ErrorCode::Control(ControlErrorCode::ProtocolInvalidEnvelope)
            }
        }
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
            validate_command_id(&status.command_id)?;
            require_enum::<CommandState>(status.state, "CommandStatus.state")
        }
        control_envelope::Body::BindingResult(result) => require_enum::<
            crate::generated::BindingResultState,
        >(
            result.state, "BindingResult.state"
        ),
        control_envelope::Body::ClientHello(_)
        | control_envelope::Body::ServerHello(_)
        | control_envelope::Body::BindingRequest(_)
        | control_envelope::Body::ServerDrain(_)
        | control_envelope::Body::ProtocolError(_) => Ok(()),
    }
}

fn validate_command(value: &Command) -> Result<(), ProtocolValidationError> {
    if value.body.is_none() {
        return MissingCommandBodySnafu.fail();
    }
    validate_command_id(&value.command_id)
}

fn validate_command_id(value: &str) -> Result<(), ProtocolValidationError> {
    if !is_canonical_command_id(value) {
        return InvalidCommandIdSnafu.fail();
    }
    Ok(())
}

/// Returns whether a command ID is one canonical lowercase RFC 4122 `UUIDv7`.
///
/// Callers that derive filesystem paths or persistence keys from a command ID must retain a
/// local check at that boundary and use this predicate instead of copying the wire rule.
#[must_use]
pub fn is_canonical_command_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| {
        uuid.get_version() == Some(Version::SortRand)
            && uuid.get_variant() == Variant::RFC4122
            && uuid.to_string() == value
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::{CommandStatus, LockSession, SessionTarget, control_envelope};

    const CANONICAL_UUID_V7: &str = "018f0e2e-8c1d-7c5e-8b12-3456789abcde";

    fn command_envelope(command_id: &str) -> ControlEnvelope {
        ControlEnvelope {
            body: Some(control_envelope::Body::Command(Command {
                command_id: command_id.to_owned(),
                created_at_unix_ms: 0,
                deadline_unix_ms: 0,
                body: Some(crate::generated::command::Body::LockSession(LockSession {
                    target: Some(SessionTarget {
                        session_instance_id: "session-1".to_owned(),
                        session_epoch: 1,
                    }),
                    requested_lock_epoch: 1,
                })),
            })),
        }
    }

    fn command_status_envelope(command_id: &str) -> ControlEnvelope {
        ControlEnvelope {
            body: Some(control_envelope::Body::CommandStatus(CommandStatus {
                command_id: command_id.to_owned(),
                state: 1,
                stable_error_code: String::new(),
            })),
        }
    }

    #[test]
    fn canonical_uuidv7_command_and_status_ids_are_accepted() {
        assert!(is_canonical_command_id(CANONICAL_UUID_V7));
        assert!(validate_envelope(&command_envelope(CANONICAL_UUID_V7)).is_ok());
        assert!(validate_envelope(&command_status_envelope(CANONICAL_UUID_V7)).is_ok());
    }

    #[test]
    fn noncanonical_command_ids_are_rejected_without_echoing_input() {
        let uppercase = CANONICAL_UUID_V7.to_uppercase();
        let braced = format!("{{{CANONICAL_UUID_V7}}}");
        let without_hyphens = CANONICAL_UUID_V7.replace('-', "");
        let invalid_ids = [
            "550e8400-e29b-41d4-a716-446655440000",
            "018f0e2e-8c1d-7c5e-0b12-3456789abcde",
            "018f0e2e-8c1d-7c5e-cb12-3456789abcde",
            "018f0e2e-8c1d-7c5e-eb12-3456789abcde",
            uppercase.as_str(),
            braced.as_str(),
            without_hyphens.as_str(),
            "not-a-command-id",
        ];

        for command_id in invalid_ids {
            assert!(!is_canonical_command_id(command_id));
            let Err(error) = validate_envelope(&command_envelope(command_id)) else {
                panic!("noncanonical command ID must be rejected");
            };
            assert!(matches!(error, ProtocolValidationError::InvalidCommandId));
            let rendered = error.to_string();
            assert!(!rendered.contains(command_id));
        }

        let invalid_status_id = "not-a-command-status-id";
        let Err(error) = validate_envelope(&command_status_envelope(invalid_status_id)) else {
            panic!("noncanonical CommandStatus ID must be rejected");
        };
        assert!(matches!(error, ProtocolValidationError::InvalidCommandId));
        let rendered = error.to_string();
        assert!(!rendered.contains(invalid_status_id));
    }

    #[test]
    fn implementation_validation_failures_collapse_to_public_control_semantics() {
        assert_eq!(
            ProtocolValidationError::InvalidCommandId.error_code(),
            ErrorCode::Control(ControlErrorCode::CommandIdInvalid)
        );

        for error in [
            ProtocolValidationError::MissingEnvelopeBody,
            ProtocolValidationError::MissingCommandBody,
            ProtocolValidationError::UnspecifiedEnum { field: "field" },
            ProtocolValidationError::UnknownEnum {
                field: "field",
                value: 99,
            },
        ] {
            assert_eq!(
                error.error_code(),
                ErrorCode::Control(ControlErrorCode::ProtocolInvalidEnvelope)
            );
        }
    }
}
