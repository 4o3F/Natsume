use axum::{
    Extension, Router,
    extract::{State, WebSocketUpgrade},
    middleware::from_fn_with_state,
    response::{IntoResponse as _, Response},
    routing::get,
};
use natsume_device_protocol::{CONTROL_MAX_FRAME_BYTES, CONTROL_SUBPROTOCOL};
use natsume_error_code::control::ControlErrorCode;

use crate::audit::CorrelationId;

use super::{AppState, error::ApiError};

mod auth;
mod dispatch;
mod registry;
mod render;
mod session;

pub(in crate::http) use self::auth::DeviceControlAuthFailureLimiter;
pub(crate) use self::registry::DeviceConnectionRegistry;
use self::{
    auth::{AuthenticatedDevice, authenticate_device_control},
    session::run_connection,
};

// These server-side values are advertised as negotiated limits in `ServerHello`.
pub(crate) const WSS_MAX_BULK_BYTES: u64 = 1_048_576;
pub(crate) const WSS_HEARTBEAT_INTERVAL_MS: u32 = 20_000;
pub(crate) const WSS_IDLE_TIMEOUT_MS: u32 = 60_000;
pub(crate) const WSS_AUTH_FAILURE_WINDOW_SECONDS: u64 = 60;
pub(crate) const WSS_AUTH_FAILURES_PER_WINDOW: u32 = 10;

/// Whether the session continues after one frame or one dispatch pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionFlow {
    Continue,
    Close(ConnectionCloseReason),
}

/// The single reason a connection winds down. Each variant also fixes the contract of whether
/// the callee already sent a frame to the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionCloseReason {
    /// Callee already sent `ServerDrain` + `Close`.
    Evicted,
    /// Callee already sent `ProtocolError` + `Close`.
    ProtocolRejected(ControlErrorCode),
    /// Callee already sent `Close`, without a `ProtocolError`.
    FrameTooLarge,
    HelloTimeout,
    IdleTimeout,
    /// The peer is gone; no frame was sent.
    PeerClosed,
    TransportFailed,
    /// Server-side fault; no frame was sent.
    StatusPersistenceFailed,
}

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    let upgrade = get(upgrade_device_control)
        .route_layer(from_fn_with_state(state, authenticate_device_control));
    Router::new().route("/device/control", upgrade)
}

async fn upgrade_device_control(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(device): Extension<AuthenticatedDevice>,
    websocket: WebSocketUpgrade,
) -> Response {
    let websocket = websocket.protocols([CONTROL_SUBPROTOCOL]);
    if websocket.selected_protocol().is_none() {
        return ApiError::device_control_subprotocol_unsupported(correlation_id).into_response();
    }
    websocket
        .max_message_size(CONTROL_MAX_FRAME_BYTES)
        .max_frame_size(CONTROL_MAX_FRAME_BYTES)
        .on_upgrade(move |socket| {
            run_connection(
                socket,
                device,
                state.database,
                state.device_connections,
                correlation_id,
            )
        })
}

#[cfg(test)]
mod tests;
