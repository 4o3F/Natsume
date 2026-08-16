use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use axum::{
    Extension, Router,
    body::Bytes,
    extract::{
        Request, State, WebSocketUpgrade,
        ws::{Message as WebSocketMessage, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse as _, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use natsume_device_protocol::{
    generated::{ControlEnvelope, ProtocolError, ServerDrain, ServerHello, control_envelope},
    validate_envelope,
};
use natsume_error_code::{ErrorCode, control::ControlErrorCode};
use prost::Message as _;
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use tokio::{
    sync::watch,
    time::{MissedTickBehavior, interval_at, sleep_until},
};
use uuid::Uuid;
use zeroize::Zeroize as _;

use crate::{
    application::enrollment::{self, DeviceConnectionEvictor},
    audit::CorrelationId,
    tls::ClientAddress,
};

use super::{AppState, error::ApiError};

pub(crate) const WSS_SUBPROTOCOL: &str = "natsume.v1";
pub(crate) const WSS_WIRE_VERSION: u32 = 1;
pub(crate) const WSS_MAX_FRAME_BYTES: usize = 65_536;
pub(crate) const WSS_MAX_BULK_BYTES: u64 = 1_048_576;
pub(crate) const WSS_HEARTBEAT_INTERVAL_MS: u32 = 20_000;
pub(crate) const WSS_IDLE_TIMEOUT_MS: u32 = 60_000;
pub(crate) const WSS_HELLO_TIMEOUT_SECONDS: u64 = 10;
pub(crate) const WSS_AUTH_FAILURE_WINDOW_SECONDS: u64 = 60;
pub(crate) const WSS_AUTH_FAILURES_PER_WINDOW: u32 = 10;

static NEXT_CONNECTION_EPOCH: AtomicU64 = AtomicU64::new(0);

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    let upgrade = get(upgrade_device_control)
        .route_layer(from_fn_with_state(state, authenticate_device_control));
    Router::new().route("/device/control", upgrade)
}

#[derive(Clone)]
pub(crate) struct DeviceConnectionRegistry {
    connections: Arc<Mutex<HashMap<String, ConnectionHandle>>>,
}

impl DeviceConnectionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn evict(&self, device_pk: &str) -> bool {
        let handle = self.lock_connections().remove(device_pk);
        let Some(handle) = handle else {
            return false;
        };
        let _send_result = handle.eviction.send(true);
        true
    }

    fn register(
        &self,
        device_pk: String,
        connection_epoch: u64,
    ) -> (ConnectionRegistration, watch::Receiver<bool>) {
        let (eviction, receiver) = watch::channel(false);
        let previous = self.lock_connections().insert(
            device_pk.clone(),
            ConnectionHandle {
                eviction,
                connection_epoch,
            },
        );
        if let Some(previous) = previous {
            let _send_result = previous.eviction.send(true);
        }
        (
            ConnectionRegistration {
                registry: self.clone(),
                device_pk,
                connection_epoch,
            },
            receiver,
        )
    }

    fn remove_if_current(&self, device_pk: &str, connection_epoch: u64) {
        let mut connections = self.lock_connections();
        if connections
            .get(device_pk)
            .is_some_and(|handle| handle.connection_epoch == connection_epoch)
        {
            connections.remove(device_pk);
        }
    }

    fn lock_connections(&self) -> MutexGuard<'_, HashMap<String, ConnectionHandle>> {
        self.connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl DeviceConnectionEvictor for DeviceConnectionRegistry {
    fn evict_device_connection(&self, device_pk: &str) -> bool {
        self.evict(device_pk)
    }
}

struct ConnectionHandle {
    eviction: watch::Sender<bool>,
    connection_epoch: u64,
}

struct ConnectionRegistration {
    registry: DeviceConnectionRegistry,
    device_pk: String,
    connection_epoch: u64,
}

impl Drop for ConnectionRegistration {
    fn drop(&mut self) {
        self.registry
            .remove_if_current(&self.device_pk, self.connection_epoch);
    }
}

#[derive(Clone)]
pub(super) struct DeviceControlAuthFailureLimiter {
    failures: Arc<Mutex<HashMap<IpAddr, (Instant, u32)>>>,
}

impl DeviceControlAuthFailureLimiter {
    pub(super) fn new() -> Self {
        Self {
            failures: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn is_limited(&self, address: IpAddr) -> bool {
        let now = Instant::now();
        let mut failures = self.lock_failures();
        prune_auth_failures(&mut failures, now);
        failures
            .get(&address)
            .is_some_and(|(_, count)| *count >= WSS_AUTH_FAILURES_PER_WINDOW)
    }

    fn record_failure(&self, address: IpAddr) {
        let now = Instant::now();
        let mut failures = self.lock_failures();
        prune_auth_failures(&mut failures, now);
        failures
            .entry(address)
            .and_modify(|(_, count)| *count = count.saturating_add(1))
            .or_insert((now, 1));
    }

    fn lock_failures(&self) -> MutexGuard<'_, HashMap<IpAddr, (Instant, u32)>> {
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn prune_auth_failures(failures: &mut HashMap<IpAddr, (Instant, u32)>, now: Instant) {
    let window = Duration::from_secs(WSS_AUTH_FAILURE_WINDOW_SECONDS);
    failures.retain(|_, (window_start, _)| now.duration_since(*window_start) < window);
}

#[derive(Clone)]
struct AuthenticatedDevice {
    device_pk: String,
    machine_hardware_id: String,
}

async fn authenticate_device_control(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    remote_address: ClientAddress,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Response {
    let source_ip = remote_address.ip();
    if state.device_control_auth_failures.is_limited(source_ip) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }

    let Some(mut token) = parse_bearer_token(&headers) else {
        state.device_control_auth_failures.record_failure(source_ip);
        return ApiError::authentication_failed(
            "device_control_authentication_failed",
            correlation_id,
        )
        .into_response();
    };
    let token_hash: [u8; 32] = Sha256::digest(token).into();
    token.zeroize();
    let lookup_hash = token_hash;
    let lookup = enrollment::device_token_authentication_facts(&state.database, lookup_hash).await;
    let row = match lookup {
        Ok(Some(row)) => row,
        Ok(None) => {
            state.device_control_auth_failures.record_failure(source_ip);
            return ApiError::authentication_failed(
                "device_control_authentication_failed",
                correlation_id,
            )
            .into_response();
        }
        Err(_) => {
            return ApiError::internal_error(
                "device_control_authentication_persistence_failed",
                correlation_id,
            )
            .into_response();
        }
    };
    if row.token_hash.len() != token_hash.len()
        || !bool::from(row.token_hash.as_slice().ct_eq(token_hash.as_slice()))
    {
        state.device_control_auth_failures.record_failure(source_ip);
        return ApiError::authentication_failed(
            "device_control_authentication_failed",
            correlation_id,
        )
        .into_response();
    }
    if !is_canonical_uuid(&row.device_pk, 7) || !is_canonical_uuid(&row.machine_hardware_id, 5) {
        return ApiError::internal_error(
            "device_control_authentication_persisted_identity_invalid",
            correlation_id,
        )
        .into_response();
    }

    request.extensions_mut().insert(AuthenticatedDevice {
        device_pk: row.device_pk,
        machine_hardware_id: row.machine_hardware_id,
    });
    next.run(request).await
}

fn parse_bearer_token(headers: &HeaderMap) -> Option<[u8; 32]> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let encoded = value.to_str().ok()?.strip_prefix("Bearer ")?;
    let bytes = encoded.as_bytes();
    if bytes.len() != 43
        || !bytes[..42]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || !matches!(
            bytes[42],
            b'A' | b'E'
                | b'I'
                | b'M'
                | b'Q'
                | b'U'
                | b'Y'
                | b'c'
                | b'g'
                | b'k'
                | b'o'
                | b's'
                | b'w'
                | b'0'
                | b'4'
                | b'8'
        )
    {
        return None;
    }
    let mut decoded = [0_u8; 32];
    let decoded_len = URL_SAFE_NO_PAD
        .decode_slice_unchecked(encoded, &mut decoded)
        .ok()?;
    (decoded_len == decoded.len()).then_some(decoded)
}

async fn upgrade_device_control(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(device): Extension<AuthenticatedDevice>,
    websocket: WebSocketUpgrade,
) -> Response {
    let websocket = websocket.protocols([WSS_SUBPROTOCOL]);
    if websocket.selected_protocol().is_none() {
        return ApiError::device_control_subprotocol_unsupported(correlation_id).into_response();
    }
    websocket
        .max_message_size(WSS_MAX_FRAME_BYTES)
        .max_frame_size(WSS_MAX_FRAME_BYTES)
        .on_upgrade(move |socket| {
            run_connection(socket, device, state.device_connections, correlation_id)
        })
}

async fn run_connection(
    mut socket: WebSocket,
    device: AuthenticatedDevice,
    registry: DeviceConnectionRegistry,
    correlation_id: CorrelationId,
) {
    let connection_epoch = next_connection_epoch();
    // Registering before the hello exchange keeps an authenticated-but-silent connection
    // visible to revocation and replacement eviction; deferring it would leave a window in
    // which a revoked token still holds a live socket.
    let (_registration, mut eviction) =
        registry.register(device.device_pk.clone(), connection_epoch);
    let Some(bytes) = wait_for_client_hello(&mut socket, &mut eviction).await else {
        return;
    };
    let envelope = match ControlEnvelope::decode(bytes) {
        Ok(envelope) if validate_envelope(&envelope).is_ok() => envelope,
        Ok(_) | Err(_) => {
            reject_protocol(&mut socket, ControlErrorCode::ProtocolInvalidEnvelope).await;
            return;
        }
    };
    let Some(control_envelope::Body::ClientHello(client_hello)) = envelope.body else {
        reject_protocol(&mut socket, ControlErrorCode::ProtocolInvalidEnvelope).await;
        return;
    };
    if client_hello.wire_version != WSS_WIRE_VERSION {
        reject_protocol(&mut socket, ControlErrorCode::ProtocolVersionUnsupported).await;
        return;
    }
    if !is_canonical_uuid(&client_hello.machine_hardware_id, 5)
        || client_hello.machine_hardware_id != device.machine_hardware_id
    {
        tracing::warn!(
            correlation_id = %correlation_id.as_text(),
            connection_epoch,
            "Device control hardware identity sanity check failed with claimed identity redacted"
        );
        reject_protocol(&mut socket, ControlErrorCode::ProtocolInvalidEnvelope).await;
        return;
    }

    let server_hello = ControlEnvelope {
        body: Some(control_envelope::Body::ServerHello(ServerHello {
            wire_version: WSS_WIRE_VERSION,
            connection_epoch,
            heartbeat_interval_ms: WSS_HEARTBEAT_INTERVAL_MS,
            idle_timeout_ms: WSS_IDLE_TIMEOUT_MS,
            max_frame_bytes: u32::try_from(WSS_MAX_FRAME_BYTES).unwrap_or(u32::MAX),
            max_bulk_bytes: WSS_MAX_BULK_BYTES,
            server_time_unix_ms: unix_time_millis_i64(),
            terminal_result_resume_cursor: 0,
            capabilities: Vec::new(),
        })),
    };
    if socket
        .send(WebSocketMessage::binary(server_hello.encode_to_vec()))
        .await
        .is_err()
    {
        return;
    }

    run_steady_session(&mut socket, &mut eviction).await;
}

/// Reads frames until the first binary one arrives, tolerating keep-alive control frames the
/// client library may emit before its hello.
async fn wait_for_client_hello(
    socket: &mut WebSocket,
    eviction: &mut watch::Receiver<bool>,
) -> Option<Bytes> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(WSS_HELLO_TIMEOUT_SECONDS);
    loop {
        let incoming = tokio::select! {
            biased;
            changed = eviction.changed() => {
                if changed.is_ok() {
                    send_server_drain(socket).await;
                }
                return None;
            }
            () = sleep_until(deadline) => {
                close_connection(socket).await;
                return None;
            }
            incoming = socket.recv() => incoming,
        };
        match incoming {
            Some(Ok(WebSocketMessage::Binary(bytes))) => {
                if bytes.len() > WSS_MAX_FRAME_BYTES {
                    close_connection(socket).await;
                    return None;
                }
                return Some(bytes);
            }
            Some(Ok(WebSocketMessage::Ping(payload))) => {
                socket.send(WebSocketMessage::Pong(payload)).await.ok()?;
            }
            Some(Ok(WebSocketMessage::Pong(_))) => {}
            Some(Ok(WebSocketMessage::Text(_))) => {
                reject_protocol(socket, ControlErrorCode::ProtocolInvalidEnvelope).await;
                return None;
            }
            Some(Ok(WebSocketMessage::Close(_)) | Err(_)) | None => return None,
        }
    }
}

async fn run_steady_session(socket: &mut WebSocket, eviction: &mut watch::Receiver<bool>) {
    let heartbeat_interval = Duration::from_millis(u64::from(WSS_HEARTBEAT_INTERVAL_MS));
    let idle_timeout = Duration::from_millis(u64::from(WSS_IDLE_TIMEOUT_MS));
    let first_ping = tokio::time::Instant::now() + heartbeat_interval;
    let mut heartbeat = interval_at(first_ping, heartbeat_interval);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_traffic = tokio::time::Instant::now();

    loop {
        tokio::select! {
            biased;
            changed = eviction.changed() => {
                if changed.is_ok() {
                    send_server_drain(socket).await;
                }
                return;
            }
            () = sleep_until(last_traffic + idle_timeout) => {
                close_connection(socket).await;
                return;
            }
            _instant = heartbeat.tick() => {
                if socket
                    .send(WebSocketMessage::Ping(Vec::new().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            incoming = socket.recv() => {
                let Some(incoming) = incoming else {
                    return;
                };
                let Ok(message) = incoming else {
                    return;
                };
                last_traffic = tokio::time::Instant::now();
                match message {
                    WebSocketMessage::Binary(bytes) => {
                        if bytes.len() > WSS_MAX_FRAME_BYTES {
                            close_connection(socket).await;
                            return;
                        }
                        let envelope = match ControlEnvelope::decode(bytes) {
                            Ok(envelope) if validate_envelope(&envelope).is_ok() => envelope,
                            Ok(_) | Err(_) => {
                                reject_protocol(
                                    socket,
                                    ControlErrorCode::ProtocolInvalidEnvelope,
                                )
                                .await;
                                return;
                            }
                        };
                        if !matches!(envelope.body, Some(control_envelope::Body::Heartbeat(_))) {
                            reject_protocol(
                                socket,
                                ControlErrorCode::ProtocolInvalidEnvelope,
                            )
                            .await;
                            return;
                        }
                    }
                    WebSocketMessage::Ping(payload) => {
                        if socket.send(WebSocketMessage::Pong(payload)).await.is_err() {
                            return;
                        }
                    }
                    WebSocketMessage::Pong(_) => {}
                    WebSocketMessage::Text(_) => {
                        reject_protocol(socket, ControlErrorCode::ProtocolInvalidEnvelope).await;
                        return;
                    }
                    WebSocketMessage::Close(_) => return,
                }
            }
        }
    }
}

async fn send_server_drain(socket: &mut WebSocket) {
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

async fn reject_protocol(socket: &mut WebSocket, code: ControlErrorCode) {
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

async fn close_connection(socket: &mut WebSocket) {
    let _send_result = socket.send(WebSocketMessage::Close(None)).await;
}

fn is_canonical_uuid(value: &str, version: usize) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| {
        uuid.get_version_num() == version && uuid.hyphenated().to_string() == value
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_token_parser_accepts_only_the_canonical_32_byte_shape() {
        let token = [0x42_u8; 32];
        let encoded = URL_SAFE_NO_PAD.encode(token);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {encoded}")
                .parse()
                .unwrap_or_else(|_| panic!("test bearer header must parse")),
        );
        assert_eq!(parse_bearer_token(&headers), Some(token));

        for malformed in [
            format!("bearer {encoded}"),
            format!("Bearer {encoded}="),
            "Bearer AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB".to_owned(),
            "Bearer !AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
        ] {
            headers.insert(
                header::AUTHORIZATION,
                malformed
                    .parse()
                    .unwrap_or_else(|_| panic!("test malformed header must parse")),
            );
            assert_eq!(parse_bearer_token(&headers), None);
        }
    }

    #[test]
    fn registry_replacement_and_epoch_checked_cleanup_preserve_the_new_slot() {
        let registry = DeviceConnectionRegistry::new();
        let (old_registration, mut old_eviction) = registry.register("device-1".to_owned(), 1);
        let (new_registration, _new_eviction) = registry.register("device-1".to_owned(), 2);
        assert!(*old_eviction.borrow_and_update());
        drop(old_registration);
        assert!(registry.evict("device-1"));
        assert!(!registry.evict("device-1"));
        drop(new_registration);
    }

    #[test]
    fn failed_authentication_limiter_blocks_after_the_frozen_count() {
        let limiter = DeviceControlAuthFailureLimiter::new();
        let address = "192.0.2.10"
            .parse()
            .unwrap_or_else(|_| panic!("test address must parse"));
        for _ in 0..WSS_AUTH_FAILURES_PER_WINDOW {
            assert!(!limiter.is_limited(address));
            limiter.record_failure(address);
        }
        assert!(limiter.is_limited(address));
    }
}
