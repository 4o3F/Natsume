use std::{fs, path::Path, time::Duration};

use futures_util::{SinkExt as _, StreamExt as _};
use natsume_device_protocol::{
    CONTROL_HELLO_TIMEOUT_SECONDS, CONTROL_MAX_FRAME_BYTES, CONTROL_WIRE_VERSION,
    generated::{ClientHello, ControlEnvelope, ServerHello, control_envelope},
    validate_envelope,
};
use natsume_error_code::ErrorCode;
use prost::Message as _;
use tokio::time::{timeout, timeout_at};
use tokio_tungstenite::tungstenite::Message as WebSocketMessage;

use crate::canonical_uuid;

use super::{
    ControlClient, ControlError,
    connect::{AttemptError, ControlSocket},
    session::{close_socket, log_protocol_error},
};

// This floor exceeds every baseline frame the client itself can emit (ClientHello and
// CommandStatus) and prevents a successful hello from making all subsequent traffic unusable.
pub(super) const CONTROL_MIN_NEGOTIATED_FRAME_BYTES: usize = 512;
pub(super) const CONTROL_IDLE_TIMEOUT_MIN_MS: u32 = 10_000;
pub(super) const CONTROL_IDLE_TIMEOUT_MAX_MS: u32 = 300_000;

#[derive(Clone, Copy)]
pub(super) struct NegotiatedLimits {
    pub(super) connection_epoch: u64,
    pub(super) heartbeat_interval_ms: u32,
    pub(super) idle_timeout: Duration,
    pub(super) max_frame_bytes: usize,
    pub(super) max_bulk_bytes: u64,
    pub(super) capability_count: usize,
}

impl ControlClient {
    pub(super) async fn connect_and_hello(
        &self,
    ) -> Result<(ControlSocket, NegotiatedLimits), AttemptError> {
        let mut socket = self.connect().await?;
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(CONTROL_HELLO_TIMEOUT_SECONDS);
        if timeout_at(
            deadline,
            socket.send(WebSocketMessage::binary(
                client_hello(&self.machine_hardware_id, &self.boot_id).encode_to_vec(),
            )),
        )
        .await
        .map_err(|_| AttemptError::Transport)?
        .is_err()
        {
            close_socket(&mut socket).await;
            return Err(AttemptError::Transport);
        }
        let hello = match timeout_at(deadline, receive_server_hello(&mut socket)).await {
            Ok(Ok(hello)) => hello,
            Ok(Err(error)) => {
                close_socket(&mut socket).await;
                return Err(error);
            }
            Err(_) => {
                close_socket(&mut socket).await;
                return Err(AttemptError::Reconnect);
            }
        };
        let limits = match NegotiatedLimits::from_server_hello(&hello) {
            Ok(limits) => limits,
            Err(error) => {
                close_socket(&mut socket).await;
                return Err(error);
            }
        };
        Ok((socket, limits))
    }
}

impl NegotiatedLimits {
    pub(super) fn from_server_hello(hello: &ServerHello) -> Result<Self, AttemptError> {
        let server_max = usize::try_from(hello.max_frame_bytes).unwrap_or(usize::MAX);
        if server_max < CONTROL_MIN_NEGOTIATED_FRAME_BYTES {
            tracing::warn!(
                negotiated_max_frame_bytes = server_max,
                minimum_max_frame_bytes = CONTROL_MIN_NEGOTIATED_FRAME_BYTES,
                "Device control peer advertised an unusable frame limit"
            );
            return Err(AttemptError::ProtocolUnsupported);
        }
        let idle_timeout_ms = hello
            .idle_timeout_ms
            .clamp(CONTROL_IDLE_TIMEOUT_MIN_MS, CONTROL_IDLE_TIMEOUT_MAX_MS);
        if idle_timeout_ms != hello.idle_timeout_ms {
            tracing::debug!(
                advertised_idle_timeout_ms = hello.idle_timeout_ms,
                honoured_idle_timeout_ms = idle_timeout_ms,
                "Device control idle timeout was clamped to the client safety range"
            );
        }
        // Heartbeat and bulk policy remain server-specific; these values are negotiated through
        // `ServerHello` rather than duplicated as client constants.
        Ok(Self {
            connection_epoch: hello.connection_epoch,
            heartbeat_interval_ms: hello.heartbeat_interval_ms,
            idle_timeout: Duration::from_millis(u64::from(idle_timeout_ms)),
            max_frame_bytes: CONTROL_MAX_FRAME_BYTES.min(server_max),
            max_bulk_bytes: hello.max_bulk_bytes,
            capability_count: hello.capabilities.len(),
        })
    }
}

pub(super) async fn receive_server_hello(
    socket: &mut ControlSocket,
) -> Result<ServerHello, AttemptError> {
    loop {
        let Some(incoming) = socket.next().await else {
            return Err(AttemptError::Reconnect);
        };
        let message = incoming.map_err(|_| AttemptError::Reconnect)?;
        match message {
            WebSocketMessage::Binary(bytes) => {
                if bytes.len() > CONTROL_MAX_FRAME_BYTES {
                    return Err(AttemptError::Reconnect);
                }
                let envelope =
                    ControlEnvelope::decode(bytes.as_ref()).map_err(|_| AttemptError::Reconnect)?;
                if validate_envelope(&envelope).is_err() {
                    return Err(AttemptError::Reconnect);
                }
                match envelope.body {
                    Some(control_envelope::Body::ServerHello(hello))
                        if hello.wire_version == CONTROL_WIRE_VERSION =>
                    {
                        return Ok(hello);
                    }
                    Some(control_envelope::Body::ServerHello(_)) => {
                        return Err(AttemptError::ProtocolUnsupported);
                    }
                    Some(control_envelope::Body::ProtocolError(error)) => {
                        log_protocol_error(&error.stable_error_code);
                        return Err(AttemptError::Reconnect);
                    }
                    Some(
                        control_envelope::Body::ClientHello(_)
                        | control_envelope::Body::Heartbeat(_)
                        | control_envelope::Body::ObservedState(_)
                        | control_envelope::Body::Command(_)
                        | control_envelope::Body::CommandStatus(_)
                        | control_envelope::Body::BindingRequest(_)
                        | control_envelope::Body::BindingResult(_)
                        | control_envelope::Body::ServerDrain(_),
                    )
                    | None => return Err(AttemptError::Reconnect),
                }
            }
            WebSocketMessage::Ping(payload) => {
                timeout(
                    Duration::from_secs(CONTROL_HELLO_TIMEOUT_SECONDS),
                    socket.send(WebSocketMessage::Pong(payload)),
                )
                .await
                .map_err(|_| AttemptError::Reconnect)?
                .map_err(|_| AttemptError::Reconnect)?;
            }
            WebSocketMessage::Pong(_) => {}
            WebSocketMessage::Text(_) | WebSocketMessage::Close(_) | WebSocketMessage::Frame(_) => {
                return Err(AttemptError::Reconnect);
            }
        }
    }
}

pub(super) fn client_hello(machine_hardware_id: &str, boot_id: &str) -> ControlEnvelope {
    ControlEnvelope {
        body: Some(control_envelope::Body::ClientHello(ClientHello {
            machine_hardware_id: machine_hardware_id.to_owned(),
            boot_id: boot_id.to_owned(),
            wire_version: CONTROL_WIRE_VERSION,
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            agent_version: String::new(),
            capabilities: Vec::new(),
            last_observed_sequence: 0,
            last_applied_generation: 0,
            last_applied_hash: Vec::new(),
        })),
    }
}

pub(super) fn read_boot_id(path: &Path) -> Result<String, ControlError> {
    let encoded = fs::read_to_string(path).map_err(|_| ControlError::BootIdentity)?;
    let value = encoded.strip_suffix('\n').unwrap_or(&encoded);
    canonical_uuid(value).ok_or(ControlError::BootIdentity)?;
    Ok(value.to_owned())
}

pub(super) fn recognized_protocol_error(value: &str) -> &'static str {
    serde_json::from_value::<ErrorCode>(serde_json::Value::String(value.to_owned()))
        .map_or("unrecognized", ErrorCode::as_str)
}
