use std::sync::Arc;

use axum::{
    Router,
    extract::{State, ws::WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use natsume_device_protocol::CONTROL_SUBPROTOCOL;

use crate::device_control::{MAX_MESSAGE_BYTES, serve_connection};

use super::super::AppState;

/// Registers the production Device Control WebSocket endpoint.
pub(in crate::http) fn routes() -> Router<AppState> {
    Router::new().route("/device/control", get(upgrade))
}

/// Accepts only an exact request for the Device Control subprotocol and applies the
/// same message-size bound used by protobuf decoding.
///
/// Protocol mismatch is rejected before upgrade. After upgrade, connection lifetime
/// and all close-on-failure semantics belong to [`serve_connection`].
async fn upgrade(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    let valid_protocol = {
        let mut protocols = upgrade.requested_protocols();
        protocols
            .next()
            .is_some_and(|value| value == CONTROL_SUBPROTOCOL)
            && protocols.next().is_none()
    };
    if !valid_protocol {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let control = Arc::clone(state.device_control());
    upgrade
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .protocols([CONTROL_SUBPROTOCOL])
        .on_upgrade(move |socket| serve_connection(socket, control))
}
