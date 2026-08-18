use axum::{
    body::Bytes,
    extract::ws::{Message as WebSocketMessage, WebSocket},
};
use natsume_device_protocol::{
    CONTROL_MAX_FRAME_BYTES,
    generated::{
        CommandState, CommandStatus, ControlEnvelope, ProtocolError, ServerDrain, control_envelope,
    },
    validate_envelope,
};
use natsume_error_code::{ErrorCode, control::ControlErrorCode};
use prost::Message as _;
use tokio::sync::Notify;

use crate::{
    application::command::{
        self, CommandError, CommandId, CommandStatusWrite, CommandStatusWriteOutcome,
        ReportedCommandState,
    },
    db::Database,
};

use super::super::{ConnectionCloseReason, ConnectionFlow};

pub(super) async fn handle_steady_binary(
    socket: &mut WebSocket,
    database: &Database,
    device_pk: &str,
    connection_epoch: u64,
    dispatch: &Notify,
    bytes: Bytes,
) -> ConnectionFlow {
    if bytes.len() > CONTROL_MAX_FRAME_BYTES {
        close_connection(socket).await;
        return ConnectionFlow::Close(ConnectionCloseReason::FrameTooLarge);
    }
    let envelope = match ControlEnvelope::decode(bytes) {
        Ok(envelope) if validate_envelope(&envelope).is_ok() => envelope,
        Ok(_) | Err(_) => {
            let code = ControlErrorCode::ProtocolInvalidEnvelope;
            reject_protocol(socket, code).await;
            return ConnectionFlow::Close(ConnectionCloseReason::ProtocolRejected(code));
        }
    };
    match envelope.body {
        Some(control_envelope::Body::Heartbeat(_)) => ConnectionFlow::Continue,
        Some(control_envelope::Body::CommandStatus(status)) => {
            let Some(status) = map_command_status(status) else {
                let code = ControlErrorCode::ProtocolInvalidEnvelope;
                reject_protocol(socket, code).await;
                return ConnectionFlow::Close(ConnectionCloseReason::ProtocolRejected(code));
            };
            match command::writeback_command_status(database, device_pk, status).await {
                Ok(CommandStatusWriteOutcome::IgnoredUnknownCommand) => {
                    tracing::debug!(
                        connection_epoch,
                        "Device reported an unknown Command; identifier redacted"
                    );
                    ConnectionFlow::Continue
                }
                Ok(CommandStatusWriteOutcome::IgnoredForeignCommand) => {
                    tracing::warn!(
                        connection_epoch,
                        "Device claimed another Device's Command; identifiers redacted"
                    );
                    ConnectionFlow::Continue
                }
                Ok(CommandStatusWriteOutcome::IgnoredRegression) => {
                    tracing::warn!(
                        connection_epoch,
                        "Device CommandStatus regressed or followed a terminal state; identifiers redacted"
                    );
                    ConnectionFlow::Continue
                }
                Ok(CommandStatusWriteOutcome::UpdatedTerminal) => {
                    // The row just left the dispatchable window, so anything queued behind
                    // the batch limit becomes visible now rather than at the next create.
                    dispatch.notify_one();
                    ConnectionFlow::Continue
                }
                Ok(
                    CommandStatusWriteOutcome::UpdatedNonterminal
                    | CommandStatusWriteOutcome::IgnoredTransition,
                ) => ConnectionFlow::Continue,
                Err(CommandError::PersistenceFailed) => {
                    tracing::error!(
                        connection_epoch,
                        "Device CommandStatus persistence failed; identifiers redacted"
                    );
                    ConnectionFlow::Close(ConnectionCloseReason::StatusPersistenceFailed)
                }
                Err(
                    CommandError::CommandIdInvalid
                    | CommandError::RequestInvalid
                    | CommandError::DeviceIdInvalid
                    | CommandError::KindInvalid
                    | CommandError::PayloadInvalid
                    | CommandError::ReasonCodeInvalid
                    | CommandError::GroupCorrelationIdInvalid
                    | CommandError::DeviceNotFound
                    | CommandError::DeviceNotEnrolled
                    | CommandError::RequestConflict
                    | CommandError::CanonicalizationFailed,
                ) => {
                    let code = ControlErrorCode::ProtocolInvalidEnvelope;
                    reject_protocol(socket, code).await;
                    ConnectionFlow::Close(ConnectionCloseReason::ProtocolRejected(code))
                }
            }
        }
        Some(
            control_envelope::Body::ClientHello(_)
            | control_envelope::Body::ServerHello(_)
            | control_envelope::Body::ObservedState(_)
            | control_envelope::Body::Command(_)
            | control_envelope::Body::BindingRequest(_)
            | control_envelope::Body::BindingResult(_)
            | control_envelope::Body::ServerDrain(_)
            | control_envelope::Body::ProtocolError(_),
        )
        | None => {
            let code = ControlErrorCode::ProtocolInvalidEnvelope;
            reject_protocol(socket, code).await;
            ConnectionFlow::Close(ConnectionCloseReason::ProtocolRejected(code))
        }
    }
}

fn map_command_status(status: CommandStatus) -> Option<CommandStatusWrite> {
    if !is_known_stable_error_code(&status.stable_error_code) {
        return None;
    }
    let command_id = CommandId::parse(&status.command_id).ok()?.value();
    let state = match CommandState::try_from(status.state) {
        Ok(CommandState::Received) => ReportedCommandState::Received,
        Ok(CommandState::Running) => ReportedCommandState::Running,
        Ok(CommandState::Succeeded) => ReportedCommandState::Succeeded,
        Ok(CommandState::Failed) => ReportedCommandState::Failed,
        Ok(CommandState::Cancelled) => ReportedCommandState::Cancelled,
        Ok(CommandState::Expired) => ReportedCommandState::Expired,
        Ok(CommandState::ManualInterventionRequired) => {
            ReportedCommandState::ManualInterventionRequired
        }
        Ok(CommandState::Unspecified) | Err(_) => return None,
    };
    let terminal_error_code =
        (!status.stable_error_code.is_empty()).then_some(status.stable_error_code);
    Some(CommandStatusWrite {
        command_id,
        state,
        terminal_error_code,
    })
}

pub(in crate::http::device_control) fn is_known_stable_error_code(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    serde_json::from_value::<ErrorCode>(serde_json::Value::String(value.to_owned())).is_ok()
}

pub(in crate::http::device_control) async fn send_server_drain(socket: &mut WebSocket) {
    let envelope = ControlEnvelope {
        body: Some(control_envelope::Body::ServerDrain(ServerDrain {
            reconnect_after_unix_ms: 0,
        })),
    };
    let _send_result = socket
        .send(WebSocketMessage::binary(envelope.encode_to_vec()))
        .await;
    close_connection(socket).await;
}

pub(super) async fn reject_protocol(socket: &mut WebSocket, code: ControlErrorCode) {
    let envelope = ControlEnvelope {
        body: Some(control_envelope::Body::ProtocolError(ProtocolError {
            stable_error_code: ErrorCode::from(code).as_str().to_owned(),
        })),
    };
    let _send_result = socket
        .send(WebSocketMessage::binary(envelope.encode_to_vec()))
        .await;
    close_connection(socket).await;
}

pub(super) async fn close_connection(socket: &mut WebSocket) {
    let _send_result = socket.send(WebSocketMessage::Close(None)).await;
}
