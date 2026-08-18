use std::{
    fs,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures_util::{Sink, SinkExt as _, StreamExt as _};
use natsume_device_protocol::{
    CONTROL_MAX_FRAME_BYTES, CONTROL_WIRE_VERSION,
    generated::{
        Command, CommandState, CommandStatus, ControlEnvelope, LockSession, ServerHello,
        SessionTarget, command, control_envelope,
    },
};
use natsume_error_code::{ErrorCode, control::ControlErrorCode};
use prost::Message as _;
use tempfile::{TempDir, tempdir};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream,
    tungstenite::{Message as WebSocketMessage, http::StatusCode, protocol::Role},
};

use super::connect::ControlSocket;
use super::{
    AttemptError, ControlClient, ControlError,
    backoff::{CONTROL_RECONNECT_MAX_SECONDS, CONTROL_RECONNECT_MIN_SECONDS, ReconnectBackoff},
    connect::{classify_upgrade_rejection, control_url},
    hello::{
        CONTROL_IDLE_TIMEOUT_MAX_MS, CONTROL_IDLE_TIMEOUT_MIN_MS,
        CONTROL_MIN_NEGOTIATED_FRAME_BYTES, NegotiatedLimits, client_hello, receive_server_hello,
    },
    session::{
        SessionOutcome, SessionProgress, reconnect_delay,
        send::{StatusSend, SteadySend, pong_outcome, send_command_status, send_steady_message},
    },
};
use crate::journal::Journal;

#[test]
fn reconnect_backoff_is_bounded_and_resets_only_after_command_progress() {
    let mut backoff = ReconnectBackoff::new();
    let sequence = (0..8).map(|_| backoff.take_delay()).collect::<Vec<_>>();
    assert_eq!(
        sequence,
        [1, 2, 4, 8, 16, 30, 30, 30].map(Duration::from_secs)
    );

    backoff.record_session_progress(SessionProgress::None);
    assert_eq!(
        backoff.take_delay(),
        Duration::from_secs(CONTROL_RECONNECT_MAX_SECONDS)
    );
    backoff.record_session_progress(SessionProgress::CommandHandled);
    assert_eq!(
        backoff.take_delay(),
        Duration::from_secs(CONTROL_RECONNECT_MIN_SECONDS)
    );
}

#[test]
fn unauthorized_forces_the_frozen_maximum_backoff() {
    let mut backoff = ReconnectBackoff::new();
    assert_eq!(backoff.take_delay(), Duration::from_secs(1));
    backoff.force_maximum();
    assert_eq!(
        backoff.take_delay(),
        Duration::from_secs(CONTROL_RECONNECT_MAX_SECONDS)
    );
    assert_eq!(
        backoff.take_delay(),
        Duration::from_secs(CONTROL_RECONNECT_MAX_SECONDS)
    );
}

#[test]
fn server_drain_delay_is_clamped_to_the_reconnect_bound() {
    assert_eq!(reconnect_delay(-1), Duration::ZERO);
    assert_eq!(reconnect_delay(1), Duration::ZERO);
    assert_eq!(
        reconnect_delay(i64::MAX),
        Duration::from_secs(CONTROL_RECONNECT_MAX_SECONDS)
    );
}

#[test]
fn client_hello_contains_only_the_phase_four_baseline() {
    let envelope = client_hello(
        "a9aa9d04-3ece-5567-8260-910930ff5e03",
        "018f0e2e-8c1d-7c5e-8b12-3456789abcde",
    );
    let Some(control_envelope::Body::ClientHello(hello)) = envelope.body else {
        panic!("client hello envelope must contain ClientHello");
    };
    assert_eq!(hello.wire_version, CONTROL_WIRE_VERSION);
    assert_eq!(hello.daemon_version, env!("CARGO_PKG_VERSION"));
    assert!(hello.agent_version.is_empty());
    assert!(hello.capabilities.is_empty());
    assert_eq!(hello.last_observed_sequence, 0);
    assert_eq!(hello.last_applied_generation, 0);
    assert!(hello.last_applied_hash.is_empty());
}

#[test]
fn negotiated_frame_floor_covers_every_client_emitted_envelope() {
    let hello = client_hello(
        "a9aa9d04-3ece-5567-8260-910930ff5e03",
        "018f0e2e-8c1d-7c5e-8b12-3456789abcde",
    );
    let status = ControlEnvelope {
        body: Some(control_envelope::Body::CommandStatus(CommandStatus {
            command_id: "018f0e2e-8c1d-7c5e-8b12-3456789abcde".to_owned(),
            state: CommandState::Failed as i32,
            stable_error_code: ErrorCode::from(ControlErrorCode::CommandPayloadConflict)
                .as_str()
                .to_owned(),
        })),
    };
    for frame in [hello.encode_to_vec(), status.encode_to_vec()] {
        assert!(frame.len() <= CONTROL_MIN_NEGOTIATED_FRAME_BYTES);
    }
}

#[test]
fn unusable_negotiated_frame_limit_is_protocol_unsupported() {
    let hello = ServerHello {
        wire_version: CONTROL_WIRE_VERSION,
        max_frame_bytes: 0,
        idle_timeout_ms: 60_000,
        ..ServerHello::default()
    };
    assert!(matches!(
        NegotiatedLimits::from_server_hello(&hello),
        Err(AttemptError::ProtocolUnsupported)
    ));
}

#[test]
fn negotiated_idle_timeout_is_clamped_to_a_closed_range() {
    for (advertised, expected) in [
        (0, CONTROL_IDLE_TIMEOUT_MIN_MS),
        (60_000, 60_000),
        (u32::MAX, CONTROL_IDLE_TIMEOUT_MAX_MS),
    ] {
        let hello = ServerHello {
            wire_version: CONTROL_WIRE_VERSION,
            max_frame_bytes: u32::try_from(CONTROL_MAX_FRAME_BYTES).unwrap_or(u32::MAX),
            idle_timeout_ms: advertised,
            ..ServerHello::default()
        };
        let limits = match NegotiatedLimits::from_server_hello(&hello) {
            Ok(limits) => limits,
            Err(error) => panic!("negotiated limits must be accepted: {error:?}"),
        };
        assert_eq!(
            limits.idle_timeout,
            Duration::from_millis(u64::from(expected))
        );
    }
}

#[test]
fn boot_id_reader_accepts_only_one_canonical_lowercase_uuid() {
    let directory = match tempdir() {
        Ok(directory) => directory,
        Err(error) => panic!("boot ID test directory must be created: {error}"),
    };
    let path = directory.path().join("boot-id");
    if let Err(error) = fs::write(&path, b"018f0e2e-8c1d-7c5e-8b12-3456789abcde\n") {
        panic!("boot ID fixture must be written: {error}");
    }
    assert!(super::read_boot_id(&path).is_ok());

    for invalid in [
        "018F0E2E-8C1D-7C5E-8B12-3456789ABCDE\n",
        "018f0e2e8c1d7c5e8b123456789abcde\n",
        "018f0e2e-8c1d-7c5e-8b12-3456789abcde\n\n",
    ] {
        if let Err(error) = fs::write(&path, invalid) {
            panic!("invalid boot ID fixture must be written: {error}");
        }
        assert!(matches!(
            super::read_boot_id(&path),
            Err(ControlError::BootIdentity)
        ));
    }
}

#[test]
fn upgrade_statuses_have_typed_retry_or_terminal_classification() {
    let unsupported = br#"{"code":"PROTOCOL_VERSION_UNSUPPORTED","status":400,"title":"redacted"}"#;
    assert_eq!(
        classify_upgrade_rejection(StatusCode::UNAUTHORIZED, None),
        AttemptError::Unauthorized
    );
    assert_eq!(
        classify_upgrade_rejection(StatusCode::TOO_MANY_REQUESTS, None),
        AttemptError::RateLimited
    );
    assert_eq!(
        classify_upgrade_rejection(StatusCode::BAD_REQUEST, Some(unsupported)),
        AttemptError::ProtocolUnsupported
    );
    assert_eq!(
        classify_upgrade_rejection(StatusCode::BAD_REQUEST, Some(b"{}")),
        AttemptError::Transport
    );
}

#[test]
fn control_url_brackets_ipv6_without_changing_the_tls_identity() {
    let endpoint = match crate::parse_endpoint("2001:db8::1", "8443") {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("IPv6 control endpoint must parse: {error}"),
    };
    assert_eq!(
        control_url(endpoint),
        "wss://[2001:db8::1]:8443/api/v2/device/control"
    );
    assert_eq!(
        std::net::SocketAddr::new(endpoint.ip(), endpoint.port().get()).ip(),
        endpoint.ip()
    );
}

struct PendingSink;

impl Sink<WebSocketMessage> for PendingSink {
    type Error = ();

    fn poll_ready(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Pending
    }

    fn start_send(self: Pin<&mut Self>, _item: WebSocketMessage) -> Result<(), Self::Error> {
        Ok(())
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Pending
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Pending
    }
}

#[tokio::test(start_paused = true)]
async fn pong_send_abandons_a_nonreading_sink_at_the_deadline() {
    let outcome = send_steady_message(
        &mut PendingSink,
        WebSocketMessage::Pong(Vec::new().into()),
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(outcome, SteadySend::TimedOut);
    assert_eq!(pong_outcome(outcome), SessionOutcome::PongTimeout);
}

#[tokio::test(start_paused = true)]
async fn command_status_send_abandons_a_nonreading_sink_at_the_deadline() {
    let outcome = send_command_status(
        &mut PendingSink,
        "018f0e2e-8c1d-7c5e-8b12-3456789abcde".to_owned(),
        CommandState::Received,
        "",
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(outcome, StatusSend::TimedOut);
}

async fn serve_silent_hello(listener: TcpListener) {
    let (stream, _address) = match listener.accept().await {
        Ok(connection) => connection,
        Err(error) => panic!("silent-peer connection must be accepted: {error}"),
    };
    let mut socket =
        WebSocketStream::from_raw_socket(MaybeTlsStream::Plain(stream), Role::Server, None).await;
    let Some(Ok(WebSocketMessage::Binary(bytes))) = socket.next().await else {
        panic!("silent peer must receive ClientHello");
    };
    let envelope = match ControlEnvelope::decode(bytes.as_ref()) {
        Ok(envelope) => envelope,
        Err(error) => panic!("ClientHello must decode: {error}"),
    };
    assert!(matches!(
        envelope.body,
        Some(control_envelope::Body::ClientHello(_))
    ));
    let hello = ControlEnvelope {
        body: Some(control_envelope::Body::ServerHello(ServerHello {
            wire_version: CONTROL_WIRE_VERSION,
            connection_epoch: 1,
            heartbeat_interval_ms: 1,
            idle_timeout_ms: 0,
            max_frame_bytes: u32::try_from(CONTROL_MAX_FRAME_BYTES).unwrap_or(u32::MAX),
            ..ServerHello::default()
        })),
    };
    if let Err(error) = socket
        .send(WebSocketMessage::binary(hello.encode_to_vec()))
        .await
    {
        panic!("silent peer must send ServerHello: {error}");
    }
    let _close = socket.next().await;
}

fn session_test_client(address: std::net::SocketAddr, directory: &TempDir) -> ControlClient {
    let journal = match Journal::open(directory.path().join("journal")) {
        Ok(journal) => journal,
        Err(error) => panic!("silent-peer journal must open: {error}"),
    };
    ControlClient {
        endpoint: "ws://127.0.0.1/".to_owned(),
        socket_address: address,
        connector: Connector::Plain,
        machine_hardware_id: "a9aa9d04-3ece-5567-8260-910930ff5e03".to_owned(),
        boot_id: "018f0e2e-8c1d-7c5e-8b12-3456789abcde".to_owned(),
        device_token: directory.path().join("unused-token"),
        journal,
        #[cfg(feature = "fixture")]
        fixture: super::FixtureState::new(),
    }
}

async fn session_socket_pair() -> (ControlSocket, ControlSocket) {
    let listener = match TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await {
        Ok(listener) => listener,
        Err(error) => panic!("session listener must bind: {error}"),
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("session listener address must be readable: {error}"),
    };
    let server = tokio::spawn(async move {
        let (stream, _address) = match listener.accept().await {
            Ok(connection) => connection,
            Err(error) => panic!("session connection must be accepted: {error}"),
        };
        WebSocketStream::from_raw_socket(MaybeTlsStream::Plain(stream), Role::Server, None).await
    });
    let stream = match TcpStream::connect(address).await {
        Ok(stream) => stream,
        Err(error) => panic!("session client must connect: {error}"),
    };
    let client =
        WebSocketStream::from_raw_socket(MaybeTlsStream::Plain(stream), Role::Client, None).await;
    let server = match server.await {
        Ok(server) => server,
        Err(error) => panic!("session server task must complete: {error}"),
    };
    (client, server)
}

fn session_limits() -> NegotiatedLimits {
    NegotiatedLimits {
        connection_epoch: 1,
        heartbeat_interval_ms: 20_000,
        idle_timeout: Duration::from_secs(1),
        max_frame_bytes: CONTROL_MAX_FRAME_BYTES,
        max_bulk_bytes: 1_048_576,
        capability_count: 0,
    }
}

fn command_envelope() -> ControlEnvelope {
    ControlEnvelope {
        body: Some(control_envelope::Body::Command(Command {
            command_id: "018f0e2e-8c1d-7c5e-8b12-3456789abcde".to_owned(),
            created_at_unix_ms: 1,
            deadline_unix_ms: 0,
            body: Some(command::Body::LockSession(LockSession {
                target: Some(SessionTarget {
                    session_instance_id: "session-1".to_owned(),
                    session_epoch: 1,
                }),
                requested_lock_epoch: 1,
            })),
        })),
    }
}

#[tokio::test]
async fn ping_and_pong_before_close_do_not_reset_backoff() {
    let (client_socket, mut server_socket) = session_socket_pair().await;
    let server = tokio::spawn(async move {
        if let Err(error) = server_socket
            .send(WebSocketMessage::Ping(Vec::new().into()))
            .await
        {
            panic!("server Ping must send: {error}");
        }
        let Some(Ok(WebSocketMessage::Pong(_))) = server_socket.next().await else {
            panic!("client must answer the server Ping");
        };
        if let Err(error) = server_socket
            .send(WebSocketMessage::Pong(Vec::new().into()))
            .await
        {
            panic!("server Pong must send: {error}");
        }
        let _close = server_socket.send(WebSocketMessage::Close(None)).await;
    });
    let directory = match tempdir() {
        Ok(directory) => directory,
        Err(error) => panic!("keep-alive journal parent must exist: {error}"),
    };
    let address = "127.0.0.1:1"
        .parse()
        .unwrap_or_else(|_| panic!("test socket address must parse"));
    let client = session_test_client(address, &directory);
    let result = client.run_session(client_socket, session_limits()).await;
    assert_eq!(result.outcome, SessionOutcome::CloseFrame);
    assert_eq!(result.progress, SessionProgress::None);

    let mut backoff = ReconnectBackoff::new();
    assert_eq!(backoff.take_delay(), Duration::from_secs(1));
    backoff.record_session_progress(result.progress);
    assert_eq!(backoff.take_delay(), Duration::from_secs(2));
    if let Err(error) = server.await {
        panic!("keep-alive server task must complete: {error}");
    }
}

#[tokio::test]
async fn receipted_command_resets_backoff() {
    let (client_socket, mut server_socket) = session_socket_pair().await;
    let server = tokio::spawn(async move {
        if let Err(error) = server_socket
            .send(WebSocketMessage::binary(command_envelope().encode_to_vec()))
            .await
        {
            panic!("server Command must send: {error}");
        }
        let Some(Ok(WebSocketMessage::Binary(bytes))) = server_socket.next().await else {
            panic!("client must return CommandStatus");
        };
        let status = match ControlEnvelope::decode(bytes.as_ref()) {
            Ok(status) => status,
            Err(error) => panic!("CommandStatus must decode: {error}"),
        };
        assert!(matches!(
            status.body,
            Some(control_envelope::Body::CommandStatus(CommandStatus {
                state,
                ..
            })) if state == CommandState::Received as i32
        ));
        let _close = server_socket.send(WebSocketMessage::Close(None)).await;
    });
    let directory = match tempdir() {
        Ok(directory) => directory,
        Err(error) => panic!("receipt journal parent must exist: {error}"),
    };
    let address = "127.0.0.1:1"
        .parse()
        .unwrap_or_else(|_| panic!("test socket address must parse"));
    let client = session_test_client(address, &directory);
    let result = client.run_session(client_socket, session_limits()).await;
    assert_eq!(result.outcome, SessionOutcome::CloseFrame);
    assert_eq!(result.progress, SessionProgress::CommandHandled);

    let mut backoff = ReconnectBackoff::new();
    assert_eq!(backoff.take_delay(), Duration::from_secs(1));
    assert_eq!(backoff.take_delay(), Duration::from_secs(2));
    backoff.record_session_progress(result.progress);
    assert_eq!(backoff.take_delay(), Duration::from_secs(1));
    if let Err(error) = server.await {
        panic!("receipt server task must complete: {error}");
    }
}

#[tokio::test(start_paused = true)]
async fn silent_peer_is_abandoned_after_completing_hello_on_a_real_socket() {
    let listener = match TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await {
        Ok(listener) => listener,
        Err(error) => panic!("silent-peer listener must bind: {error}"),
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("silent-peer address must be readable: {error}"),
    };
    let server = tokio::spawn(serve_silent_hello(listener));

    let stream = match TcpStream::connect(address).await {
        Ok(stream) => stream,
        Err(error) => panic!("silent-peer client must connect: {error}"),
    };
    let mut socket =
        WebSocketStream::from_raw_socket(MaybeTlsStream::Plain(stream), Role::Client, None).await;
    if let Err(error) = socket
        .send(WebSocketMessage::binary(
            client_hello(
                "a9aa9d04-3ece-5567-8260-910930ff5e03",
                "018f0e2e-8c1d-7c5e-8b12-3456789abcde",
            )
            .encode_to_vec(),
        ))
        .await
    {
        panic!("ClientHello must send: {error}");
    }
    let hello = match receive_server_hello(&mut socket).await {
        Ok(hello) => hello,
        Err(error) => panic!("ServerHello must complete: {error:?}"),
    };
    let limits = match NegotiatedLimits::from_server_hello(&hello) {
        Ok(limits) => limits,
        Err(error) => panic!("ServerHello limits must negotiate: {error:?}"),
    };
    let directory = match tempdir() {
        Ok(directory) => directory,
        Err(error) => panic!("silent-peer journal parent must exist: {error}"),
    };
    let client = session_test_client(address, &directory);
    let session = tokio::spawn(async move { client.run_session(socket, limits).await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(u64::from(
        CONTROL_IDLE_TIMEOUT_MIN_MS,
    )))
    .await;
    let result = match session.await {
        Ok(result) => result,
        Err(error) => panic!("silent-peer session task must complete: {error}"),
    };
    assert_eq!(result.outcome, SessionOutcome::IdleTimeout);
    assert_eq!(result.progress, SessionProgress::None);
    if let Err(error) = server.await {
        panic!("silent-peer server task must complete: {error}");
    }
}
