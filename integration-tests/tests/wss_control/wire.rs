use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt as _, StreamExt as _};
use natsume_device_protocol::{
    CONTROL_MAX_FRAME_BYTES, CONTROL_SUBPROTOCOL, CONTROL_WIRE_VERSION,
    generated::{ClientHello, ControlEnvelope, control_envelope},
};
use natsume_integration_tests::harness::{TestServer, require_ok};
use prost::Message as _;
use rustls::{ClientConfig, RootCertStore, pki_types::CertificateDer};
use tokio::{
    net::TcpStream,
    time::{timeout, timeout_at},
};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{
        Error as WebSocketError, Message as WebSocketMessage,
        client::IntoClientRequest as _,
        handshake::client::Response as WebSocketResponse,
        http::{HeaderValue, header as ws_header},
    },
};
use uuid::Uuid;

const WSS_EVENT_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) type TestWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug)]
struct NoCertificateResolver;

impl rustls::client::ResolvesClientCert for NoCertificateResolver {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _signature_schemes: &[rustls::SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        None
    }

    fn has_certs(&self) -> bool {
        false
    }
}

fn websocket_connector(server: &TestServer) -> Connector {
    let mut roots = RootCertStore::empty();
    require_ok(
        roots.add(CertificateDer::from(
            server.control_certificate_der().to_vec(),
        )),
        "control root must parse for rustls",
    );
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = require_ok(
        ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13]),
        "rustls protocol policy must build",
    );
    let mut config = builder
        .with_root_certificates(roots)
        .with_client_cert_resolver(Arc::new(NoCertificateResolver));
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Connector::Rustls(Arc::new(config))
}

pub(super) async fn websocket_attempt(
    server: &TestServer,
    token: Option<&str>,
    subprotocol: Option<&str>,
    cookie: Option<&str>,
) -> Result<(TestWebSocket, WebSocketResponse), WebSocketError> {
    let mut request = require_ok(
        server.websocket_url().into_client_request(),
        "WebSocket request must build",
    );
    if let Some(token) = token {
        request.headers_mut().insert(
            ws_header::AUTHORIZATION,
            require_ok(
                HeaderValue::from_str(&format!("Bearer {token}")),
                "bearer header must parse",
            ),
        );
    }
    if let Some(subprotocol) = subprotocol {
        request.headers_mut().insert(
            ws_header::SEC_WEBSOCKET_PROTOCOL,
            require_ok(
                HeaderValue::from_str(subprotocol),
                "subprotocol header must parse",
            ),
        );
    }
    if let Some(cookie) = cookie {
        request.headers_mut().insert(
            ws_header::COOKIE,
            require_ok(HeaderValue::from_str(cookie), "cookie header must parse"),
        );
    }
    connect_async_tls_with_config(request, None, false, Some(websocket_connector(server))).await
}

pub(super) async fn open_websocket(server: &TestServer, token: &str) -> TestWebSocket {
    let (socket, response) = require_ok(
        websocket_attempt(server, Some(token), Some(CONTROL_SUBPROTOCOL), None).await,
        "authenticated WebSocket upgrade must succeed",
    );
    assert_eq!(response.status().as_u16(), 101);
    assert_eq!(
        response
            .headers()
            .get(ws_header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok()),
        Some(CONTROL_SUBPROTOCOL)
    );
    socket
}

pub(super) async fn connect_and_hello(
    server: &TestServer,
    token: &str,
    hardware_id: Uuid,
) -> (TestWebSocket, u64) {
    let mut socket = open_websocket(server, token).await;
    let before = unix_time_millis();
    send_envelope(&mut socket, client_hello(hardware_id, CONTROL_WIRE_VERSION)).await;
    let envelope = receive_envelope(&mut socket).await;
    let after = unix_time_millis();
    let Some(control_envelope::Body::ServerHello(hello)) = envelope.body else {
        panic!("ClientHello must receive ServerHello");
    };
    assert_eq!(hello.wire_version, CONTROL_WIRE_VERSION);
    assert!(hello.connection_epoch > 0);
    assert_eq!(hello.heartbeat_interval_ms, 20_000);
    assert_eq!(hello.idle_timeout_ms, 60_000);
    assert_eq!(
        hello.max_frame_bytes,
        u32::try_from(CONTROL_MAX_FRAME_BYTES).unwrap_or(u32::MAX)
    );
    assert_eq!(hello.max_bulk_bytes, 1_048_576);
    assert!(hello.server_time_unix_ms >= before && hello.server_time_unix_ms <= after);
    assert!(hello.capabilities.is_empty());
    (socket, hello.connection_epoch)
}

pub(super) async fn send_envelope(socket: &mut TestWebSocket, envelope: ControlEnvelope) {
    require_ok(
        socket
            .send(WebSocketMessage::Binary(envelope.encode_to_vec().into()))
            .await,
        "control envelope must be sent",
    );
}

pub(super) async fn receive_envelope(socket: &mut TestWebSocket) -> ControlEnvelope {
    receive_envelope_with_bytes(socket).await.1
}

pub(super) async fn receive_envelope_with_bytes(
    socket: &mut TestWebSocket,
) -> (Vec<u8>, ControlEnvelope) {
    receive_envelope_with_bytes_timeout(socket, WSS_EVENT_TIMEOUT).await
}

pub(super) async fn receive_envelope_with_timeout(
    socket: &mut TestWebSocket,
    wait: Duration,
) -> ControlEnvelope {
    receive_envelope_with_bytes_timeout(socket, wait).await.1
}

async fn receive_envelope_with_bytes_timeout(
    socket: &mut TestWebSocket,
    wait: Duration,
) -> (Vec<u8>, ControlEnvelope) {
    loop {
        let message = require_ok(
            timeout(wait, socket.next()).await,
            "control envelope must arrive within the bounded wait",
        );
        let message = require_ok(message.ok_or(()), "WebSocket must remain open");
        match require_ok(message, "WebSocket message must be readable") {
            WebSocketMessage::Binary(bytes) => {
                let envelope = require_ok(
                    ControlEnvelope::decode(bytes.as_ref()),
                    "binary frame must decode as a control envelope",
                );
                return (bytes.to_vec(), envelope);
            }
            WebSocketMessage::Ping(payload) => {
                require_ok(
                    socket.send(WebSocketMessage::Pong(payload)).await,
                    "server ping must be answered",
                );
            }
            WebSocketMessage::Pong(_) | WebSocketMessage::Frame(_) => {}
            WebSocketMessage::Text(_) | WebSocketMessage::Close(_) => {
                panic!("control envelope must arrive before text or close")
            }
        }
    }
}

/// Drains keep-alive traffic for the whole window and fails on any control envelope, so a
/// heartbeat Ping does not masquerade as a re-dispatched command.
pub(super) async fn assert_no_binary_frame_for(socket: &mut TestWebSocket, window: Duration) {
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let Ok(message) = timeout_at(deadline, socket.next()).await else {
            return;
        };
        let message = require_ok(
            require_ok(message.ok_or(()), "WebSocket must remain open"),
            "WebSocket message must be readable",
        );
        assert!(
            !matches!(message, WebSocketMessage::Binary(_)),
            "a completed dispatch pass must not be repeated on the heartbeat"
        );
    }
}

pub(super) async fn expect_no_control_envelope(socket: &mut TestWebSocket) {
    assert!(
        timeout(Duration::from_millis(300), socket.next())
            .await
            .is_err(),
        "held sync_secret must have zero wire effect"
    );
}

pub(super) fn assert_command_id(envelope: &ControlEnvelope, expected_command_id: Uuid) {
    let Some(control_envelope::Body::Command(command)) = envelope.body.as_ref() else {
        panic!("dispatch must produce a Command envelope");
    };
    assert_eq!(command.command_id, expected_command_id.to_string());
}

pub(super) async fn expect_protocol_error(socket: &mut TestWebSocket, expected_code: &str) {
    let envelope = receive_envelope(socket).await;
    let Some(control_envelope::Body::ProtocolError(error)) = envelope.body else {
        panic!("protocol rejection must send ProtocolError");
    };
    assert_eq!(error.stable_error_code, expected_code);
    expect_closed(socket).await;
}

pub(super) async fn expect_server_drain_and_close(socket: &mut TestWebSocket) {
    let envelope = receive_envelope(socket).await;
    let Some(control_envelope::Body::ServerDrain(drain)) = envelope.body else {
        panic!("registry eviction must send ServerDrain");
    };
    assert_eq!(drain.reconnect_after_unix_ms, 0);
    expect_closed(socket).await;
}

pub(super) async fn assert_ping_round_trip(socket: &mut TestWebSocket) {
    let payload = vec![1_u8, 2, 3];
    require_ok(
        socket
            .send(WebSocketMessage::Ping(payload.clone().into()))
            .await,
        "client ping must be sent",
    );
    loop {
        let message = require_ok(
            timeout(WSS_EVENT_TIMEOUT, socket.next()).await,
            "pong must arrive within the bounded wait",
        );
        let message = require_ok(message.ok_or(()), "WebSocket must remain open for pong");
        match require_ok(message, "pong frame must be readable") {
            WebSocketMessage::Pong(received) => {
                assert_eq!(received.as_ref(), payload.as_slice());
                return;
            }
            WebSocketMessage::Ping(received) => {
                require_ok(
                    socket.send(WebSocketMessage::Pong(received)).await,
                    "server ping must be answered",
                );
            }
            WebSocketMessage::Frame(_) => {}
            WebSocketMessage::Binary(_)
            | WebSocketMessage::Text(_)
            | WebSocketMessage::Close(_) => {
                panic!("pong must arrive before another application or close frame")
            }
        }
    }
}

pub(super) async fn close_client(socket: &mut TestWebSocket) {
    let _close_result = socket.close(None).await;
    expect_closed(socket).await;
}

pub(super) async fn expect_closed(socket: &mut TestWebSocket) {
    require_ok(
        timeout(WSS_EVENT_TIMEOUT, async {
            loop {
                match socket.next().await {
                    None
                    | Some(
                        Err(
                            WebSocketError::ConnectionClosed
                            | WebSocketError::AlreadyClosed
                            | WebSocketError::Io(_)
                            | WebSocketError::Protocol(_),
                        )
                        | Ok(WebSocketMessage::Close(_)),
                    ) => return,
                    Some(Ok(WebSocketMessage::Ping(payload))) => {
                        let _send_result = socket.send(WebSocketMessage::Pong(payload)).await;
                    }
                    Some(
                        Ok(
                            WebSocketMessage::Binary(_)
                            | WebSocketMessage::Text(_)
                            | WebSocketMessage::Pong(_)
                            | WebSocketMessage::Frame(_),
                        )
                        | Err(_),
                    ) => {}
                }
            }
        })
        .await,
        "WebSocket must close within the bounded wait",
    );
}

pub(super) fn client_hello(machine_hardware_id: Uuid, wire_version: u32) -> ControlEnvelope {
    ControlEnvelope {
        body: Some(control_envelope::Body::ClientHello(ClientHello {
            machine_hardware_id: machine_hardware_id.to_string(),
            boot_id: "018f0e2e-8c1d-7c5e-8b12-3456789abcde".to_owned(),
            wire_version,
            daemon_version: "2.0.0".to_owned(),
            agent_version: "2.0.0".to_owned(),
            capabilities: Vec::new(),
            last_observed_sequence: 0,
            last_applied_generation: 0,
            last_applied_hash: Vec::new(),
        })),
    }
}

fn unix_time_millis() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}
