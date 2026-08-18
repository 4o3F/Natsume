use std::time::Duration;

use futures_util::{Sink, SinkExt as _};
use natsume_device_protocol::generated::{
    CommandState, CommandStatus, ControlEnvelope, control_envelope,
};
use prost::Message as _;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as WebSocketMessage;

use super::SessionOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::control) enum StatusSend {
    Sent,
    TransportLost,
    TimedOut,
}

pub(in crate::control) async fn send_command_status<S>(
    socket: &mut S,
    command_id: String,
    state: CommandState,
    stable_error_code: &str,
    send_timeout: Duration,
) -> StatusSend
where
    S: Sink<WebSocketMessage> + Unpin,
{
    let envelope = ControlEnvelope {
        body: Some(control_envelope::Body::CommandStatus(CommandStatus {
            command_id,
            state: state as i32,
            stable_error_code: stable_error_code.to_owned(),
        })),
    };
    match send_steady_message(
        socket,
        WebSocketMessage::binary(envelope.encode_to_vec()),
        send_timeout,
    )
    .await
    {
        SteadySend::Sent => StatusSend::Sent,
        SteadySend::TransportLost => StatusSend::TransportLost,
        SteadySend::TimedOut => StatusSend::TimedOut,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::control) enum SteadySend {
    Sent,
    TransportLost,
    TimedOut,
}

pub(in crate::control) async fn send_steady_message<S>(
    socket: &mut S,
    message: WebSocketMessage,
    send_timeout: Duration,
) -> SteadySend
where
    S: Sink<WebSocketMessage> + Unpin,
{
    match timeout(send_timeout, socket.send(message)).await {
        Ok(Ok(())) => SteadySend::Sent,
        Ok(Err(_)) => SteadySend::TransportLost,
        Err(_) => SteadySend::TimedOut,
    }
}

pub(in crate::control) const fn pong_outcome(send: SteadySend) -> SessionOutcome {
    match send {
        SteadySend::Sent => SessionOutcome::FrameHandled,
        SteadySend::TransportLost => SessionOutcome::TransportLost,
        SteadySend::TimedOut => SessionOutcome::PongTimeout,
    }
}
