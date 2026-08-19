//! Batch 0 feasibility only: a private listener proves ordinary WSS can carry a bounded
//! Ed25519 Challenge -> Proof -> exact `ClientInit` exchange without changing production routes.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use futures_util::{SinkExt as _, StreamExt as _};
use rustls::{ProtocolVersion, pki_types::ServerName};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_tungstenite::{
    WebSocketStream, accept_hdr_async_with_config, client_async_with_config,
    tungstenite::{
        Message as WebSocketMessage,
        client::IntoClientRequest as _,
        handshake::server::{ErrorResponse, Request, Response},
        http::{HeaderValue, StatusCode, header},
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};

#[path = "ordinary_wss_ed25519_feasibility/protocol.rs"]
mod protocol;
#[path = "ordinary_wss_ed25519_feasibility/tls.rs"]
mod tls;

const ALPN_HTTP_1_1: &[u8] = b"http/1.1";
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_REASON: &str = "batch-0-complete";
const AUTH_FAILURE_CLOSE_REASON: &str = "authentication-failed";
const MAX_TEST_MESSAGE_BYTES: usize = 1024;

#[derive(Clone, Copy)]
enum ExchangeMode {
    Valid,
    BadSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StageError {
    UnexpectedMessageKind,
    Transport,
    Closed,
}

#[derive(Debug, PartialEq, Eq)]
enum StageMessage {
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong,
}

#[test]
fn preauth_stage_allows_control_frames_and_rejects_wrong_kinds() {
    assert_eq!(
        classify_stage_message(WebSocketMessage::Ping(vec![1].into())),
        Ok(StageMessage::Ping(vec![1]))
    );
    assert_eq!(
        classify_stage_message(WebSocketMessage::Pong(Vec::new().into())),
        Ok(StageMessage::Pong)
    );
    for message in [
        WebSocketMessage::Text("unexpected".into()),
        WebSocketMessage::Close(None),
    ] {
        assert_eq!(
            classify_stage_message(message),
            Err(StageError::UnexpectedMessageKind)
        );
    }
}

#[tokio::test]
async fn ordinary_wss_ed25519_challenge_response_is_feasible() {
    protocol::assert_deterministic_vectors();
    run_exchange(ExchangeMode::Valid).await;
}

#[tokio::test]
async fn bad_signature_is_closed_before_client_init() {
    run_exchange(ExchangeMode::BadSignature).await;
}

async fn run_exchange(mode: ExchangeMode) {
    let (server_config, client_config) = tls::configs();
    let listener = require_ok(
        TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await,
        "private feasibility listener must bind",
    );
    let address = require_ok(
        listener.local_addr(),
        "private feasibility listener address must be readable",
    );

    let exchange = async {
        let ((), ()) = tokio::join!(
            run_server(listener, TlsAcceptor::from(server_config), mode),
            run_client(address, TlsConnector::from(client_config), mode),
        );
    };
    require_ok(
        Box::pin(timeout(EXCHANGE_TIMEOUT, exchange)).await,
        "bounded ordinary-WSS feasibility exchange timed out",
    );
}

async fn run_server(listener: TcpListener, acceptor: TlsAcceptor, mode: ExchangeMode) {
    let (tcp, _peer) = require_ok(
        listener.accept().await,
        "private feasibility TCP connection must be accepted",
    );
    let tls = require_ok(
        acceptor.accept(tcp).await,
        "private feasibility TLS handshake must succeed",
    );
    assert_eq!(
        tls.get_ref().1.protocol_version(),
        Some(ProtocolVersion::TLSv1_3)
    );
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(ALPN_HTTP_1_1));

    let mut socket = require_ok(
        accept_hdr_async_with_config(tls, select_subprotocol, Some(websocket_config())).await,
        "private feasibility WebSocket upgrade must succeed",
    );

    let challenge = protocol::random_challenge();
    send_binary(&mut socket, protocol::encode_challenge(challenge)).await;

    let proof_bytes = require_ok(
        receive_binary(&mut socket).await,
        "proof stage must receive one binary message",
    );
    let proof = require_ok(
        protocol::decode_proof(&proof_bytes),
        "proof message must have the exact test shape",
    );
    if matches!(mode, ExchangeMode::BadSignature) {
        assert_eq!(
            protocol::verify_proof(challenge, &proof),
            Err(protocol::ProofError::SignatureInvalid)
        );
        close_server(&mut socket, AUTH_FAILURE_CLOSE_REASON).await;
        return;
    }
    require_ok(
        protocol::verify_proof(challenge, &proof),
        "client proof must verify strictly",
    );

    let init_bytes = require_ok(
        receive_binary(&mut socket).await,
        "ClientInit stage must receive one binary message",
    );
    let _init = require_ok(
        protocol::verify_client_init(&proof, &init_bytes),
        "ClientInit must match the signed proof",
    );

    send_binary(&mut socket, vec![protocol::ACCEPTED_TAG]).await;
    close_server(&mut socket, CLOSE_REASON).await;
}

async fn run_client(address: SocketAddr, connector: TlsConnector, mode: ExchangeMode) {
    let tcp = require_ok(
        TcpStream::connect(address).await,
        "private feasibility TCP client must connect",
    );
    let tls = require_ok(
        connector
            .connect(ServerName::from(IpAddr::V4(Ipv4Addr::LOCALHOST)), tcp)
            .await,
        "server-auth TLS must validate the private test root and IP SAN",
    );
    assert_eq!(
        tls.get_ref().1.protocol_version(),
        Some(ProtocolVersion::TLSv1_3)
    );
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(ALPN_HTTP_1_1));

    let mut request = require_ok(
        format!("wss://{address}{}", protocol::CONTROL_ROUTE).into_client_request(),
        "private feasibility WebSocket request must build",
    );
    request.headers_mut().insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(protocol::CONTROL_SUBPROTOCOL),
    );
    let (mut socket, response) = require_ok(
        client_async_with_config(request, tls, Some(websocket_config())).await,
        "ordinary WebSocket client handshake must succeed",
    );
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(
        response
            .headers()
            .get(header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok()),
        Some(protocol::CONTROL_SUBPROTOCOL)
    );

    let challenge_bytes = require_ok(
        receive_binary(&mut socket).await,
        "challenge stage must receive one binary message",
    );
    let challenge = require_ok(
        protocol::decode_challenge(&challenge_bytes),
        "challenge message must have the exact test shape",
    );
    assert_eq!(challenge.protocol_version, protocol::PROTOCOL_VERSION);
    let signing_key = protocol::deterministic_signing_key();
    let init = protocol::client_init(signing_key.verifying_key().to_bytes());
    let init_bytes = protocol::encode_client_init(&init);
    let mut proof = protocol::sign_proof(
        &signing_key,
        challenge,
        &init,
        protocol::sha256(&init_bytes),
    );
    if matches!(mode, ExchangeMode::BadSignature) {
        proof.signature[0] ^= 1;
    }

    let proof_bytes = protocol::encode_proof(&proof);
    send_binary(&mut socket, proof_bytes).await;
    if matches!(mode, ExchangeMode::BadSignature) {
        finish_client_close(&mut socket, AUTH_FAILURE_CLOSE_REASON).await;
        return;
    }
    send_binary(&mut socket, init_bytes).await;
    assert_eq!(
        require_ok(
            receive_binary(&mut socket).await,
            "acceptance stage must receive one binary message",
        ),
        vec![protocol::ACCEPTED_TAG]
    );

    finish_client_close(&mut socket, CLOSE_REASON).await;
}

async fn close_server<S>(socket: &mut WebSocketStream<S>, reason: &'static str)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    require_ok(
        socket
            .close(Some(CloseFrame {
                code: CloseCode::Normal,
                reason: reason.into(),
            }))
            .await,
        "server must initiate a normal WebSocket close",
    );
    assert_clean_close_with_reason(receive_message(socket).await, reason);
    require_ok(
        socket.get_mut().shutdown().await,
        "server must send TLS close_notify",
    );
    require_tls_eof(socket.get_mut()).await;
}

async fn finish_client_close<S>(socket: &mut WebSocketStream<S>, reason: &'static str)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    assert_clean_close_with_reason(receive_message(socket).await, reason);
    require_ok(
        socket.flush().await,
        "client must flush the WebSocket close reply",
    );
    require_ok(
        socket.get_mut().shutdown().await,
        "client must send TLS close_notify",
    );
    require_tls_eof(socket.get_mut()).await;
}

async fn require_tls_eof<S>(stream: &mut S)
where
    S: AsyncRead + Unpin,
{
    let mut byte = [0_u8; 1];
    assert_eq!(
        require_ok(stream.read(&mut byte).await, "TLS shutdown read failed"),
        0,
        "peer must send TLS close_notify before EOF"
    );
}

fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_TEST_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_TEST_MESSAGE_BYTES))
}

#[allow(clippy::result_large_err, clippy::unnecessary_wraps)]
fn select_subprotocol(
    request: &Request,
    mut response: Response,
) -> Result<Response, ErrorResponse> {
    assert_eq!(request.uri().path(), protocol::CONTROL_ROUTE);
    assert!(request.uri().query().is_none());
    assert!(request.headers().get(header::AUTHORIZATION).is_none());
    assert_eq!(
        request
            .headers()
            .get(header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok()),
        Some(protocol::CONTROL_SUBPROTOCOL)
    );
    response.headers_mut().insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(protocol::CONTROL_SUBPROTOCOL),
    );
    Ok(response)
}

async fn send_binary<S>(socket: &mut WebSocketStream<S>, bytes: Vec<u8>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    require_ok(
        socket.send(WebSocketMessage::Binary(bytes.into())).await,
        "standalone binary message must send",
    );
}

async fn receive_binary<S>(socket: &mut WebSocketStream<S>) -> Result<Vec<u8>, StageError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let stage = match socket.next().await {
            Some(Ok(message)) => classify_stage_message(message)?,
            Some(Err(_)) => return Err(StageError::Transport),
            None => return Err(StageError::Closed),
        };
        match stage {
            StageMessage::Binary(bytes) => return Ok(bytes),
            StageMessage::Ping(payload) => socket
                .send(WebSocketMessage::Pong(payload.into()))
                .await
                .map_err(|_| StageError::Transport)?,
            StageMessage::Pong => {}
        }
    }
}

fn classify_stage_message(message: WebSocketMessage) -> Result<StageMessage, StageError> {
    match message {
        WebSocketMessage::Binary(bytes) => Ok(StageMessage::Binary(bytes.to_vec())),
        WebSocketMessage::Ping(payload) => Ok(StageMessage::Ping(payload.to_vec())),
        WebSocketMessage::Pong(_) => Ok(StageMessage::Pong),
        WebSocketMessage::Text(_) | WebSocketMessage::Close(_) | WebSocketMessage::Frame(_) => {
            Err(StageError::UnexpectedMessageKind)
        }
    }
}

async fn receive_message<S>(socket: &mut WebSocketStream<S>) -> WebSocketMessage
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match socket.next().await {
        Some(Ok(message)) => message,
        Some(Err(error)) => {
            drop(error);
            panic!("private feasibility WebSocket read failed");
        }
        None => panic!("private feasibility WebSocket closed early"),
    }
}

fn assert_clean_close_with_reason(message: WebSocketMessage, reason: &str) {
    let WebSocketMessage::Close(Some(frame)) = message else {
        panic!("peer must send one explicit close frame");
    };
    assert_eq!(frame.code, CloseCode::Normal);
    assert_eq!(frame.reason.as_str(), reason);
}

fn require_ok<T, E>(result: Result<T, E>, message: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            drop(error);
            panic!("{message}");
        }
    }
}
