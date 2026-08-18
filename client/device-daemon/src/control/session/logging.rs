use super::SessionOutcome;
use crate::control::hello::recognized_protocol_error;

pub(in crate::control) fn log_protocol_error(value: &str) {
    let stable_error_code = recognized_protocol_error(value);
    tracing::warn!(
        stable_error_code,
        "Device control peer reported a protocol error"
    );
}

pub(in crate::control) fn log_session_outcome(outcome: SessionOutcome) {
    match outcome {
        SessionOutcome::FrameHandled
        | SessionOutcome::CommandHandled
        | SessionOutcome::ProtocolError => {}
        SessionOutcome::ServerDrain(delay) => tracing::info!(
            reconnect_delay_seconds = delay.as_secs(),
            "Device control peer requested a drained reconnect"
        ),
        SessionOutcome::IdleTimeout => tracing::warn!(
            session_end = "idle_timeout",
            "Device control peer sent no frame before the negotiated read deadline"
        ),
        SessionOutcome::StreamEnded => tracing::warn!(
            session_end = "stream_end",
            "Device control stream ended before a close frame"
        ),
        SessionOutcome::TransportLost => tracing::warn!(
            session_end = "transport_error",
            "Device control session transport was lost"
        ),
        SessionOutcome::PongTimeout => tracing::warn!(
            session_end = "pong_timeout",
            "Device control peer stopped reading before the Pong send deadline"
        ),
        SessionOutcome::TextFrame => tracing::warn!(
            session_end = "text_frame",
            "Device control peer sent a forbidden text frame"
        ),
        SessionOutcome::CloseFrame => tracing::warn!(
            session_end = "peer_close",
            "Device control peer closed the session"
        ),
        SessionOutcome::UnexpectedWebSocketFrame => tracing::warn!(
            session_end = "unexpected_websocket_frame",
            "Device control peer produced an unexpected raw WebSocket frame"
        ),
        SessionOutcome::FrameTooLarge => tracing::warn!(
            session_end = "frame_too_large",
            "Device control peer exceeded the negotiated frame limit"
        ),
        SessionOutcome::DecodeFailed => tracing::warn!(
            session_end = "decode_failed",
            "Device control peer sent an undecodable binary envelope"
        ),
        SessionOutcome::ValidationFailed => tracing::warn!(
            session_end = "validation_failed",
            "Device control peer sent an invalid envelope"
        ),
        SessionOutcome::UnexpectedEnvelope => tracing::warn!(
            session_end = "unexpected_envelope",
            "Device control peer sent an envelope forbidden in the steady session"
        ),
        SessionOutcome::CommandIdRejected => tracing::warn!(
            session_end = "command_id_rejected",
            "Device command journal rejected a command identifier; identifier redacted"
        ),
        SessionOutcome::JournalUnusable => tracing::error!(
            session_end = "journal_unusable",
            "Device command journal persistence is unavailable; command facts redacted"
        ),
        SessionOutcome::JournalTaskFailed => tracing::error!(
            session_end = "journal_task_failed",
            "Device command journal worker failed; command facts redacted"
        ),
        SessionOutcome::CommandStatusTransportLost(_) => tracing::warn!(
            session_end = "command_status_transport_lost",
            "Device command status could not be sent; command facts redacted"
        ),
        SessionOutcome::CommandStatusTimeout(_) => tracing::warn!(
            session_end = "command_status_timeout",
            "Device control peer stopped reading before the CommandStatus send deadline"
        ),
    }
}
