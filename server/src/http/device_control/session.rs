use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use axum::{
    body::Bytes,
    extract::ws::{Message as WebSocketMessage, WebSocket},
};
use natsume_device_protocol::{
    CONTROL_HELLO_TIMEOUT_SECONDS, CONTROL_MAX_FRAME_BYTES, CONTROL_WIRE_VERSION,
    generated::{ControlEnvelope, ServerHello, control_envelope},
    validate_envelope,
};
use natsume_error_code::control::ControlErrorCode;
use prost::Message as _;
use tokio::{
    sync::{Notify, watch},
    time::{MissedTickBehavior, interval_at, sleep_until},
};
use uuid::Uuid;

use crate::{audit::CorrelationId, db::Database};

use super::{
    ConnectionCloseReason, ConnectionFlow, WSS_HEARTBEAT_INTERVAL_MS, WSS_IDLE_TIMEOUT_MS,
    WSS_MAX_BULK_BYTES,
    auth::AuthenticatedDevice,
    dispatch::CommandDispatcher,
    registry::{DeviceConnectionRegistry, DisplacedConnection, RegisteredConnection},
};

mod frame;

#[cfg(test)]
pub(super) use self::frame::is_known_stable_error_code;
pub(super) use self::frame::send_server_drain;
use self::frame::{close_connection, handle_steady_binary, reject_protocol};

static NEXT_CONNECTION_EPOCH: AtomicU64 = AtomicU64::new(0);

pub(super) async fn run_connection(
    mut socket: WebSocket,
    device: AuthenticatedDevice,
    database: Database,
    registry: DeviceConnectionRegistry,
    correlation_id: CorrelationId,
) {
    let connection_epoch = next_connection_epoch();
    let device_pk = device.device_pk.as_text();
    // Registering before the hello exchange keeps an authenticated-but-silent connection
    // visible to revocation and replacement eviction; deferring it would leave a window in
    // which a revoked token still holds a live socket.
    let RegisteredConnection {
        registration: _registration,
        mut eviction,
        dispatch,
        displaced,
    } = registry.register(device_pk.clone(), connection_epoch);

    let close_reason = 'connection: {
        let bytes = match wait_for_client_hello(&mut socket, &mut eviction).await {
            Ok(bytes) => bytes,
            Err(reason) => break 'connection reason,
        };
        let envelope = match ControlEnvelope::decode(bytes) {
            Ok(envelope) if validate_envelope(&envelope).is_ok() => envelope,
            Ok(_) | Err(_) => {
                let code = ControlErrorCode::ProtocolInvalidEnvelope;
                reject_protocol(&mut socket, code).await;
                break 'connection ConnectionCloseReason::ProtocolRejected(code);
            }
        };
        let Some(control_envelope::Body::ClientHello(client_hello)) = envelope.body else {
            let code = ControlErrorCode::ProtocolInvalidEnvelope;
            reject_protocol(&mut socket, code).await;
            break 'connection ConnectionCloseReason::ProtocolRejected(code);
        };
        if client_hello.wire_version != CONTROL_WIRE_VERSION {
            let code = ControlErrorCode::ProtocolVersionUnsupported;
            reject_protocol(&mut socket, code).await;
            break 'connection ConnectionCloseReason::ProtocolRejected(code);
        }
        if !is_canonical_uuid(&client_hello.machine_hardware_id, 5)
            || client_hello.machine_hardware_id != device.machine_hardware_id
        {
            tracing::warn!(
                correlation_id = %correlation_id.as_text(),
                connection_epoch,
                "Device control hardware identity sanity check failed with claimed identity redacted"
            );
            let code = ControlErrorCode::ProtocolInvalidEnvelope;
            reject_protocol(&mut socket, code).await;
            break 'connection ConnectionCloseReason::ProtocolRejected(code);
        }

        let server_hello = ControlEnvelope {
            body: Some(control_envelope::Body::ServerHello(ServerHello {
                wire_version: CONTROL_WIRE_VERSION,
                connection_epoch,
                heartbeat_interval_ms: WSS_HEARTBEAT_INTERVAL_MS,
                idle_timeout_ms: WSS_IDLE_TIMEOUT_MS,
                max_frame_bytes: u32::try_from(CONTROL_MAX_FRAME_BYTES).unwrap_or(u32::MAX),
                max_bulk_bytes: WSS_MAX_BULK_BYTES,
                server_time_unix_ms: unix_time_millis_i64(),
                capabilities: Vec::new(),
            })),
        };
        if socket
            .send(WebSocketMessage::binary(server_hello.encode_to_vec()))
            .await
            .is_err()
        {
            break 'connection ConnectionCloseReason::TransportFailed;
        }

        break 'connection SteadySession {
            socket,
            eviction,
            dispatch_signal: dispatch,
            database,
            device_pk,
            connection_epoch,
            dispatcher: CommandDispatcher::new(),
        }
        .run()
        .await;
    };

    tracing::info!(
        correlation_id = %correlation_id.as_text(),
        connection_epoch,
        close_reason = ?close_reason,
        displaced_live_connection = displaced == DisplacedConnection::Evicted,
        "Device control connection closed"
    );
}

fn is_canonical_uuid(value: &str, version: usize) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| {
        uuid.get_version_num() == version && uuid.hyphenated().to_string() == value
    })
}

/// Reads frames until the first binary one arrives, tolerating keep-alive control frames the
/// client library may emit before its hello.
pub(super) async fn wait_for_client_hello(
    socket: &mut WebSocket,
    eviction: &mut watch::Receiver<bool>,
) -> Result<Bytes, ConnectionCloseReason> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(CONTROL_HELLO_TIMEOUT_SECONDS);
    loop {
        let incoming = tokio::select! {
            biased;
            changed = eviction.changed() => {
                if changed.is_ok() {
                    send_server_drain(socket).await;
                    return Err(ConnectionCloseReason::Evicted);
                }
                return Err(ConnectionCloseReason::TransportFailed);
            }
            () = sleep_until(deadline) => {
                close_connection(socket).await;
                return Err(ConnectionCloseReason::HelloTimeout);
            }
            incoming = socket.recv() => incoming,
        };
        match incoming {
            Some(Ok(WebSocketMessage::Binary(bytes))) => {
                if bytes.len() > CONTROL_MAX_FRAME_BYTES {
                    close_connection(socket).await;
                    return Err(ConnectionCloseReason::FrameTooLarge);
                }
                return Ok(bytes);
            }
            Some(Ok(WebSocketMessage::Ping(payload))) => {
                require_transport_send(socket.send(WebSocketMessage::Pong(payload)).await)?;
            }
            Some(Ok(WebSocketMessage::Pong(_))) => {}
            Some(Ok(WebSocketMessage::Text(_))) => {
                let code = ControlErrorCode::ProtocolInvalidEnvelope;
                reject_protocol(socket, code).await;
                return Err(ConnectionCloseReason::ProtocolRejected(code));
            }
            Some(Ok(WebSocketMessage::Close(_))) | None => {
                return Err(ConnectionCloseReason::PeerClosed);
            }
            Some(Err(_)) => return Err(ConnectionCloseReason::TransportFailed),
        }
    }
}

struct SteadySession {
    socket: WebSocket,
    eviction: watch::Receiver<bool>,
    dispatch_signal: Arc<Notify>,
    database: Database,
    device_pk: String,
    connection_epoch: u64,
    dispatcher: CommandDispatcher,
}

impl SteadySession {
    async fn run(mut self) -> ConnectionCloseReason {
        if let ConnectionFlow::Close(reason) = self
            .dispatcher
            .dispatch_now(
                &mut self.socket,
                &self.database,
                &self.device_pk,
                &mut self.eviction,
            )
            .await
        {
            return reason;
        }

        let heartbeat_interval = Duration::from_millis(u64::from(WSS_HEARTBEAT_INTERVAL_MS));
        let idle_timeout = Duration::from_millis(u64::from(WSS_IDLE_TIMEOUT_MS));
        let first_ping = tokio::time::Instant::now() + heartbeat_interval;
        let mut heartbeat = interval_at(first_ping, heartbeat_interval);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut last_traffic = tokio::time::Instant::now();

        loop {
            tokio::select! {
                biased;
                changed = self.eviction.changed() => {
                    if changed.is_ok() {
                        send_server_drain(&mut self.socket).await;
                        return ConnectionCloseReason::Evicted;
                    }
                    return ConnectionCloseReason::TransportFailed;
                }
                () = self.dispatch_signal.notified() => {
                    if let ConnectionFlow::Close(reason) = self
                        .dispatcher
                        .dispatch_now(
                            &mut self.socket,
                            &self.database,
                            &self.device_pk,
                            &mut self.eviction,
                        )
                        .await
                    {
                        return reason;
                    }
                }
                () = sleep_until(last_traffic + idle_timeout) => {
                    close_connection(&mut self.socket).await;
                    return ConnectionCloseReason::IdleTimeout;
                }
                _instant = heartbeat.tick() => {
                    if let ConnectionFlow::Close(reason) = self
                        .dispatcher
                        .retry_if_owed(
                            &mut self.socket,
                            &self.database,
                            &self.device_pk,
                            &mut self.eviction,
                        )
                        .await
                    {
                        return reason;
                    }
                    if self
                        .socket
                        .send(WebSocketMessage::Ping(Vec::new().into()))
                        .await
                        .is_err()
                    {
                        return ConnectionCloseReason::TransportFailed;
                    }
                }
                incoming = self.socket.recv() => {
                    let Some(incoming) = incoming else {
                        return ConnectionCloseReason::PeerClosed;
                    };
                    let Ok(message) = incoming else {
                        return ConnectionCloseReason::TransportFailed;
                    };
                    last_traffic = tokio::time::Instant::now();
                    if let ConnectionFlow::Close(reason) = self.handle_message(message).await {
                        return reason;
                    }
                }
            }
        }
    }

    async fn handle_message(&mut self, message: WebSocketMessage) -> ConnectionFlow {
        match message {
            WebSocketMessage::Binary(bytes) => {
                handle_steady_binary(
                    &mut self.socket,
                    &self.database,
                    &self.device_pk,
                    self.connection_epoch,
                    &self.dispatch_signal,
                    bytes,
                )
                .await
            }
            WebSocketMessage::Ping(payload) => {
                if self
                    .socket
                    .send(WebSocketMessage::Pong(payload))
                    .await
                    .is_err()
                {
                    ConnectionFlow::Close(ConnectionCloseReason::TransportFailed)
                } else {
                    ConnectionFlow::Continue
                }
            }
            WebSocketMessage::Pong(_) => ConnectionFlow::Continue,
            WebSocketMessage::Text(_) => {
                let code = ControlErrorCode::ProtocolInvalidEnvelope;
                reject_protocol(&mut self.socket, code).await;
                ConnectionFlow::Close(ConnectionCloseReason::ProtocolRejected(code))
            }
            WebSocketMessage::Close(_) => ConnectionFlow::Close(ConnectionCloseReason::PeerClosed),
        }
    }
}

fn require_transport_send<T, E>(result: Result<T, E>) -> Result<T, ConnectionCloseReason> {
    result.map_err(|_| ConnectionCloseReason::TransportFailed)
}

fn next_connection_epoch() -> u64 {
    let seed = unix_time_millis_u64().max(1);
    let _initialization =
        NEXT_CONNECTION_EPOCH.compare_exchange(0, seed, Ordering::SeqCst, Ordering::SeqCst);
    NEXT_CONNECTION_EPOCH.fetch_add(1, Ordering::SeqCst)
}

fn unix_time_millis_i64() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

fn unix_time_millis_u64() -> u64 {
    u64::try_from(unix_time_millis_i64()).unwrap_or(0)
}
