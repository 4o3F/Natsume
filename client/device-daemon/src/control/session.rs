use std::time::{Duration, SystemTime};

use futures_util::{SinkExt as _, StreamExt as _};
use natsume_device_protocol::{
    CONTROL_HELLO_TIMEOUT_SECONDS,
    generated::{CommandState, ControlEnvelope, control_envelope},
    validate_envelope,
};
use natsume_error_code::{ErrorCode, control::ControlErrorCode};
use prost::Message as _;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::{Bytes, Message as WebSocketMessage};

use crate::journal::{JournalError, JournalOutcome};

use super::{
    ControlClient, backoff::CONTROL_RECONNECT_MAX_SECONDS, connect::ControlSocket,
    hello::NegotiatedLimits,
};

mod logging;
pub(super) mod send;

pub(super) use self::logging::{log_protocol_error, log_session_outcome};
use self::send::{StatusSend, pong_outcome, send_command_status, send_steady_message};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandHandling {
    Receipted,
    ConflictReported,
    IdRejected,
    JournalUnusable,
    JournalTaskFailed,
    TransportLost(SessionProgress),
    SendTimedOut(SessionProgress),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionProgress {
    None,
    CommandHandled,
}

impl SessionProgress {
    const fn merge(self, next: Self) -> Self {
        match (self, next) {
            (Self::CommandHandled, _) | (_, Self::CommandHandled) => Self::CommandHandled,
            (Self::None, Self::None) => Self::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionOutcome {
    FrameHandled,
    CommandHandled,
    ServerDrain(Duration),
    IdleTimeout,
    StreamEnded,
    TransportLost,
    PongTimeout,
    TextFrame,
    CloseFrame,
    UnexpectedWebSocketFrame,
    FrameTooLarge,
    DecodeFailed,
    ValidationFailed,
    UnexpectedEnvelope,
    ProtocolError,
    CommandIdRejected,
    JournalUnusable,
    JournalTaskFailed,
    CommandStatusTransportLost(SessionProgress),
    CommandStatusTimeout(SessionProgress),
}

pub(super) struct SessionResult {
    pub(super) outcome: SessionOutcome,
    pub(super) progress: SessionProgress,
}

impl SessionResult {
    pub(super) const fn requested_delay(&self) -> Duration {
        match self.outcome {
            SessionOutcome::ServerDrain(delay) => delay,
            SessionOutcome::FrameHandled
            | SessionOutcome::CommandHandled
            | SessionOutcome::IdleTimeout
            | SessionOutcome::StreamEnded
            | SessionOutcome::TransportLost
            | SessionOutcome::PongTimeout
            | SessionOutcome::TextFrame
            | SessionOutcome::CloseFrame
            | SessionOutcome::UnexpectedWebSocketFrame
            | SessionOutcome::FrameTooLarge
            | SessionOutcome::DecodeFailed
            | SessionOutcome::ValidationFailed
            | SessionOutcome::UnexpectedEnvelope
            | SessionOutcome::ProtocolError
            | SessionOutcome::CommandIdRejected
            | SessionOutcome::JournalUnusable
            | SessionOutcome::JournalTaskFailed
            | SessionOutcome::CommandStatusTransportLost(_)
            | SessionOutcome::CommandStatusTimeout(_) => Duration::ZERO,
        }
    }
}

impl SessionOutcome {
    const fn progress(self) -> SessionProgress {
        match self {
            Self::CommandHandled => SessionProgress::CommandHandled,
            Self::CommandStatusTransportLost(progress) | Self::CommandStatusTimeout(progress) => {
                progress
            }
            Self::FrameHandled
            | Self::ServerDrain(_)
            | Self::IdleTimeout
            | Self::StreamEnded
            | Self::TransportLost
            | Self::PongTimeout
            | Self::TextFrame
            | Self::CloseFrame
            | Self::UnexpectedWebSocketFrame
            | Self::FrameTooLarge
            | Self::DecodeFailed
            | Self::ValidationFailed
            | Self::UnexpectedEnvelope
            | Self::ProtocolError
            | Self::CommandIdRejected
            | Self::JournalUnusable
            | Self::JournalTaskFailed => SessionProgress::None,
        }
    }
}

impl ControlClient {
    pub(super) async fn run_session(
        &self,
        mut socket: ControlSocket,
        limits: NegotiatedLimits,
    ) -> SessionResult {
        let mut progress = SessionProgress::None;
        loop {
            let Ok(incoming) = timeout(limits.idle_timeout, socket.next()).await else {
                close_socket(&mut socket).await;
                return SessionResult {
                    outcome: SessionOutcome::IdleTimeout,
                    progress,
                };
            };
            let Some(incoming) = incoming else {
                return SessionResult {
                    outcome: SessionOutcome::StreamEnded,
                    progress,
                };
            };
            let Ok(message) = incoming else {
                close_socket(&mut socket).await;
                return SessionResult {
                    outcome: SessionOutcome::TransportLost,
                    progress,
                };
            };
            let outcome = match message {
                WebSocketMessage::Binary(bytes) => {
                    self.handle_binary(&mut socket, &limits, bytes).await
                }
                WebSocketMessage::Ping(payload) => pong_outcome(
                    send_steady_message(
                        &mut socket,
                        WebSocketMessage::Pong(payload),
                        limits.idle_timeout,
                    )
                    .await,
                ),
                WebSocketMessage::Pong(_) => SessionOutcome::FrameHandled,
                WebSocketMessage::Text(_) => SessionOutcome::TextFrame,
                WebSocketMessage::Close(_) => SessionOutcome::CloseFrame,
                WebSocketMessage::Frame(_) => SessionOutcome::UnexpectedWebSocketFrame,
            };
            progress = progress.merge(outcome.progress());
            if matches!(
                outcome,
                SessionOutcome::FrameHandled | SessionOutcome::CommandHandled
            ) {
                continue;
            }
            close_socket(&mut socket).await;
            return SessionResult { outcome, progress };
        }
    }

    async fn handle_binary(
        &self,
        socket: &mut ControlSocket,
        limits: &NegotiatedLimits,
        bytes: Bytes,
    ) -> SessionOutcome {
        if bytes.len() > limits.max_frame_bytes {
            return SessionOutcome::FrameTooLarge;
        }
        let Ok(envelope) = ControlEnvelope::decode(bytes.as_ref()) else {
            return SessionOutcome::DecodeFailed;
        };
        if validate_envelope(&envelope).is_err() {
            return SessionOutcome::ValidationFailed;
        }
        match envelope.body {
            Some(control_envelope::Body::Command(command)) => {
                match self
                    .handle_command(socket, command.command_id, bytes, limits.idle_timeout)
                    .await
                {
                    CommandHandling::Receipted | CommandHandling::ConflictReported => {
                        SessionOutcome::CommandHandled
                    }
                    CommandHandling::IdRejected => SessionOutcome::CommandIdRejected,
                    CommandHandling::JournalUnusable => SessionOutcome::JournalUnusable,
                    CommandHandling::JournalTaskFailed => SessionOutcome::JournalTaskFailed,
                    CommandHandling::TransportLost(progress) => {
                        SessionOutcome::CommandStatusTransportLost(progress)
                    }
                    CommandHandling::SendTimedOut(progress) => {
                        SessionOutcome::CommandStatusTimeout(progress)
                    }
                }
            }
            Some(control_envelope::Body::ServerDrain(drain)) => {
                SessionOutcome::ServerDrain(reconnect_delay(drain.reconnect_after_unix_ms))
            }
            Some(control_envelope::Body::ProtocolError(error)) => {
                log_protocol_error(&error.stable_error_code);
                SessionOutcome::ProtocolError
            }
            Some(
                control_envelope::Body::ClientHello(_)
                | control_envelope::Body::ServerHello(_)
                | control_envelope::Body::Heartbeat(_)
                | control_envelope::Body::ObservedState(_)
                | control_envelope::Body::CommandStatus(_)
                | control_envelope::Body::BindingRequest(_)
                | control_envelope::Body::BindingResult(_),
            )
            | None => SessionOutcome::UnexpectedEnvelope,
        }
    }

    async fn handle_command(
        &self,
        socket: &mut ControlSocket,
        command_id: String,
        bytes: Bytes,
        send_timeout: Duration,
    ) -> CommandHandling {
        let journal_command_id = command_id.clone();
        let journal = self.journal.clone();
        let frame_bytes = bytes.to_vec();
        let outcome =
            tokio::task::spawn_blocking(move || journal.record(&journal_command_id, &frame_bytes))
                .await;
        let outcome = match outcome {
            Err(_) => return CommandHandling::JournalTaskFailed,
            Ok(Err(JournalError::InvalidCommandId)) => return CommandHandling::IdRejected,
            Ok(Err(JournalError::Unavailable)) => return CommandHandling::JournalUnusable,
            Ok(Ok(outcome)) => outcome,
        };
        #[cfg(feature = "fixture")]
        self.fixture.record_journaled_command();
        let (state, stable_error_code, success, persisted_progress) = match outcome {
            JournalOutcome::Recorded | JournalOutcome::AlreadyRecorded => (
                CommandState::Received,
                "",
                CommandHandling::Receipted,
                SessionProgress::CommandHandled,
            ),
            JournalOutcome::Conflict => (
                CommandState::Failed,
                ErrorCode::from(ControlErrorCode::CommandPayloadConflict).as_str(),
                CommandHandling::ConflictReported,
                SessionProgress::None,
            ),
        };
        match send_command_status(socket, command_id, state, stable_error_code, send_timeout).await
        {
            StatusSend::Sent => success,
            StatusSend::TransportLost => CommandHandling::TransportLost(persisted_progress),
            StatusSend::TimedOut => CommandHandling::SendTimedOut(persisted_progress),
        }
    }
}

pub(super) fn reconnect_delay(reconnect_after_unix_ms: i64) -> Duration {
    let target = u128::try_from(reconnect_after_unix_ms).unwrap_or(0);
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let remaining = target.saturating_sub(now);
    let requested = Duration::from_millis(u64::try_from(remaining).unwrap_or(u64::MAX));
    let maximum = Duration::from_secs(CONTROL_RECONNECT_MAX_SECONDS);
    if requested > maximum {
        tracing::debug!(
            requested_delay_seconds = requested.as_secs(),
            honoured_delay_seconds = maximum.as_secs(),
            "Device control ServerDrain reconnect delay was clamped"
        );
        maximum
    } else {
        requested
    }
}

pub(super) async fn close_socket(socket: &mut ControlSocket) {
    let _result = timeout(
        Duration::from_secs(CONTROL_HELLO_TIMEOUT_SECONDS),
        socket.send(WebSocketMessage::Close(None)),
    )
    .await;
}
