use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use diesel::{Connection, QueryableByName, RunQueryDsl, sql_types::Text, sqlite::SqliteConnection};
use futures_util::{SinkExt as _, StreamExt as _};
use natsume_device_daemon::enrollment::{
    EnrollmentClient, EnrollmentPaths, EnrollmentStep, EnrollmentWaitState,
};
use natsume_device_protocol::generated::{
    ClientHello, ControlEnvelope, GatewayState, Heartbeat, HomeState, ObservedStateSnapshot,
    SecretState, SessionLockState, SessionState, StateApplyStatus, control_envelope,
};
use natsume_machine_identity::EvidenceQuality;
use natsume_server::{commands, config::ServerConfig};
use prost::Message as _;
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use reqwest::{StatusCode, redirect::Policy};
use rustls::{ClientConfig, RootCertStore, pki_types::CertificateDer};
use serde_json::Value;
use tempfile::{TempDir, tempdir};
use tokio::{
    net::TcpStream,
    sync::oneshot,
    task::JoinHandle,
    time::{sleep, timeout},
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
use zeroize::{Zeroize as _, Zeroizing};

use natsume_integration_tests::harness::bootstrap_operator;

const LOCALHOST: Ipv4Addr = Ipv4Addr::LOCALHOST;
const GATEWAY_HOSTNAME: &str = "gateway.contest.example";
const TEST_NAMESPACE: Uuid = Uuid::from_u128(0x2234_5678_1234_5678_9234_5678_1234_5678);
const SERVER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const WSS_EVENT_TIMEOUT: Duration = Duration::from_secs(5);
const OPERATOR_LOGIN: &str = "wp3-admin";
const OPERATOR_PASSWORD: &str = "wp3-operator-password";
const WSS_SUBPROTOCOL: &str = "natsume.v1";
const WSS_MAX_FRAME_BYTES: usize = 65_536;

type TestWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct TestServer {
    directory: TempDir,
    address: SocketAddr,
    database_path: PathBuf,
    control_certificate_der: Vec<u8>,
    control_root_path: PathBuf,
    origin_root_path: PathBuf,
    operator: Option<OperatorSession>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<bool>>,
}

struct OperatorSession {
    http: reqwest::Client,
    cookie: Zeroizing<String>,
}

struct ClientFixture {
    client: EnrollmentClient,
    keys_directory: PathBuf,
}

struct ServerPki {
    private_directory: PathBuf,
    server_certificate_path: PathBuf,
    server_key_path: PathBuf,
    control_root_path: PathBuf,
    origin_root_path: PathBuf,
    control_certificate_der: Vec<u8>,
}

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

#[derive(QueryableByName)]
struct AuditDetailRow {
    #[diesel(sql_type = Text)]
    redacted_detail_json: String,
}

impl ClientFixture {
    async fn enroll(&self) {
        let step = require_ok(self.client.step().await, "Enrollment step must complete");
        assert_eq!(step, EnrollmentStep::Enrolled);
    }

    fn token(&self) -> String {
        require_ok(
            fs::read_to_string(self.keys_directory.join("device-token")),
            "issued Device Token must be readable",
        )
    }
}

impl TestServer {
    async fn start() -> Self {
        let directory = require_ok(tempdir(), "server test directory must be created");
        let pki = install_server_pki(directory.path());
        let database_path = directory.path().join("server.sqlite3");
        let vault_key_path = pki.private_directory.join("server-root.key");
        let site_path = directory.path().join("site.toml");
        require_ok(
            fs::write(
                &site_path,
                format!(
                    "gateway_hostname = \"{GATEWAY_HOSTNAME}\"\n\
                     gateway_not_after = \"4090-01-01T00:00:00Z\"\n\
                     contest_end = \"4089-12-31T00:00:00Z\"\n"
                ),
            ),
            "server site configuration must be written",
        );

        let reservation = require_ok(
            StdTcpListener::bind(SocketAddr::from((LOCALHOST, 0))),
            "server port must be reserved",
        );
        let address = require_ok(
            reservation.local_addr(),
            "server reserved address must be readable",
        );
        drop(reservation);
        let server_config_path = directory.path().join("server.toml");
        require_ok(
            fs::write(
                &server_config_path,
                format!(
                    "[listen]\nhttps = \"{address}\"\n\
                     [storage]\ndatabase = \"{}\"\nroot_key = \"{}\"\n\
                     [tls]\ncertificate = \"{}\"\nprivate_key = \"{}\"\n\
                     [site]\nconfig = \"{}\"\ncontrol_root = \"{}\"\nlocal_origin_root = \"{}\"\n",
                    database_path.display(),
                    vault_key_path.display(),
                    pki.server_certificate_path.display(),
                    pki.server_key_path.display(),
                    site_path.display(),
                    pki.control_root_path.display(),
                    pki.origin_root_path.display(),
                ),
            ),
            "server configuration must be written",
        );
        bootstrap_operator(
            env!("CARGO_BIN_EXE_server-bootstrap-driver"),
            &server_config_path,
            OPERATOR_LOGIN,
            OPERATOR_PASSWORD,
        )
        .await;
        let config = require_ok(
            ServerConfig::load_from(&server_config_path),
            "server configuration must load",
        );
        let (shutdown, shutdown_signal) = oneshot::channel();
        let task = tokio::spawn(async move {
            commands::run_until(config, async move {
                let _result = shutdown_signal.await;
            })
            .await
            .is_ok()
        });

        let mut server = Self {
            directory,
            address,
            database_path,
            control_certificate_der: pki.control_certificate_der,
            control_root_path: pki.control_root_path,
            origin_root_path: pki.origin_root_path,
            operator: None,
            shutdown: Some(shutdown),
            task: Some(task),
        };
        server.wait_until_ready().await;
        server.operator = Some(server.login_operator().await);
        server
    }

    async fn wait_until_ready(&self) {
        let client = self.control_http_client();
        let url = self.api_url("/api/v2/health");
        for _ in 0..100 {
            if let Ok(response) = client.get(&url).send().await
                && response.status() == StatusCode::OK
            {
                return;
            }
            assert!(
                !self.task.as_ref().is_some_and(JoinHandle::is_finished),
                "real TLS server stopped before becoming ready"
            );
            sleep(Duration::from_millis(20)).await;
        }
        panic!("real TLS server did not become ready");
    }

    fn control_http_client(&self) -> reqwest::Client {
        let root = require_ok(
            reqwest::Certificate::from_der(&self.control_certificate_der),
            "control root must parse for reqwest",
        );
        require_ok(
            reqwest::Client::builder()
                .tls_backend_rustls()
                .tls_certs_only([root])
                .https_only(true)
                .redirect(Policy::none())
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(5))
                .build(),
            "control HTTP client must build",
        )
    }

    fn websocket_connector(&self) -> Connector {
        let mut roots = RootCertStore::empty();
        require_ok(
            roots.add(CertificateDer::from(self.control_certificate_der.clone())),
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

    fn api_url(&self, path: &str) -> String {
        format!("https://{}{path}", self.address)
    }

    fn websocket_url(&self) -> String {
        format!("wss://{}/api/v2/device/control", self.address)
    }

    async fn websocket_attempt(
        &self,
        token: Option<&str>,
        subprotocol: Option<&str>,
        cookie: Option<&str>,
    ) -> Result<(TestWebSocket, WebSocketResponse), WebSocketError> {
        let mut request = require_ok(
            self.websocket_url().into_client_request(),
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
        connect_async_tls_with_config(request, None, false, Some(self.websocket_connector())).await
    }

    async fn login_operator(&self) -> OperatorSession {
        let http = self.control_http_client();
        let response = require_ok(
            http.post(self.api_url("/api/v2/session"))
                .json(&serde_json::json!({
                    "login_name": OPERATOR_LOGIN,
                    "password": OPERATOR_PASSWORD,
                }))
                .send()
                .await,
            "operator login must complete",
        );
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = require_ok(
            response
                .headers()
                .get(reqwest::header::SET_COOKIE)
                .ok_or(()),
            "operator login must issue a session cookie",
        );
        let set_cookie = require_ok(set_cookie.to_str(), "operator session cookie must be text");
        let cookie = require_ok(
            set_cookie.split(';').next().ok_or(()),
            "operator session cookie pair must exist",
        );
        OperatorSession {
            http,
            cookie: Zeroizing::new(cookie.to_owned()),
        }
    }

    fn operator(&self) -> &OperatorSession {
        self.operator
            .as_ref()
            .unwrap_or_else(|| panic!("operator session must be established"))
    }

    fn operator_request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let operator = self.operator();
        operator
            .http
            .request(method, self.api_url(path))
            .header(reqwest::header::COOKIE, operator.cookie.as_str())
    }

    async fn open_window(&self) {
        let response = require_ok(
            self.operator_request(
                reqwest::Method::POST,
                "/api/v2/provisioning-window/actions/open",
            )
            .send()
            .await,
            "provisioning-window open request must complete",
        );
        assert_eq!(response.status(), StatusCode::OK);
    }

    fn client(&self, name: &str, machine_hardware_id: Uuid) -> ClientFixture {
        let keys_directory = self.directory.path().join(format!("client-{name}-keys"));
        require_ok(
            fs::create_dir(&keys_directory),
            "client keys directory must be created",
        );
        require_ok(
            fs::set_permissions(&keys_directory, fs::Permissions::from_mode(0o700)),
            "client keys directory mode must be set",
        );
        let client_config = self.directory.path().join(format!("client-{name}.toml"));
        require_ok(
            fs::write(
                &client_config,
                format!(
                    "[server]\nip = \"{}\"\nport = {}\n",
                    self.address.ip(),
                    self.address.port()
                ),
            ),
            "client endpoint configuration must be written",
        );
        let paths = EnrollmentPaths::new(
            client_config,
            self.control_root_path.clone(),
            self.origin_root_path.clone(),
            keys_directory.clone(),
        );
        let client = require_ok(
            EnrollmentClient::prepare(
                paths,
                machine_hardware_id,
                EvidenceQuality::Medium,
                GATEWAY_HOSTNAME.to_owned(),
            ),
            "Enrollment client must prepare",
        );
        ClientFixture {
            client,
            keys_directory,
        }
    }

    async fn approve_request(&self, request_id: Uuid) {
        let path = format!("/api/v2/enrollment-requests/{request_id}/actions/approve");
        let response = require_ok(
            self.operator_request(reqwest::Method::POST, &path)
                .send()
                .await,
            "Enrollment approval must complete",
        );
        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn only_device_id(&self) -> String {
        let response = require_ok(
            self.operator_request(reqwest::Method::GET, "/api/v2/devices")
                .send()
                .await,
            "Device list must complete",
        );
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = require_ok(response.json().await, "Device list must be JSON");
        let devices = value
            .as_array()
            .unwrap_or_else(|| panic!("Device list must be an array"));
        assert_eq!(devices.len(), 1);
        devices[0]
            .get("device_id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("Device ID must be present"))
            .to_owned()
    }

    async fn revoke_device(&self, device_id: &str) {
        let path = format!("/api/v2/devices/{device_id}/actions/revoke");
        let response = require_ok(
            self.operator_request(reqwest::Method::POST, &path)
                .send()
                .await,
            "Device revocation must complete",
        );
        assert_eq!(response.status(), StatusCode::OK);
    }

    fn issuance_eviction_flags(&self) -> Vec<bool> {
        let path = self
            .database_path
            .to_str()
            .unwrap_or_else(|| panic!("database path must be UTF-8"));
        let mut connection = require_ok(
            SqliteConnection::establish(path),
            "audit observer must connect",
        );
        let rows = require_ok(
            diesel::sql_query(
                "SELECT redacted_detail_json FROM audit_events \
                 WHERE action_kind = 'issue_device_credentials' ORDER BY rowid",
            )
            .load::<AuditDetailRow>(&mut connection),
            "issuance audit rows must be readable",
        );
        rows.into_iter()
            .map(|row| {
                let detail: Value = require_ok(
                    serde_json::from_str(&row.redacted_detail_json),
                    "issuance audit detail must be JSON",
                );
                detail
                    .get("evicted_live_connection")
                    .and_then(Value::as_bool)
                    .unwrap_or_else(|| panic!("issuance eviction evidence must be boolean"))
            })
            .collect()
    }

    async fn shutdown(mut self) {
        let sender = self
            .shutdown
            .take()
            .unwrap_or_else(|| panic!("server shutdown sender must exist"));
        assert!(sender.send(()).is_ok(), "server shutdown signal must send");
        let task = self
            .task
            .take()
            .unwrap_or_else(|| panic!("server task must exist"));
        match timeout(SERVER_SHUTDOWN_TIMEOUT, task).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false) | Err(_)) | Err(_) => panic!("real TLS server must stop cleanly"),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _result = sender.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[tokio::test]
async fn upgrade_auth_subprotocol_revocation_and_operator_separation_are_exact() {
    let server = TestServer::start().await;
    server.open_window().await;
    let hardware_id = machine_id(b"upgrade-auth");
    let client = server.client("upgrade-auth", hardware_id);
    client.enroll().await;
    let token = client.token();
    let wrong_token = URL_SAFE_NO_PAD.encode([0xa5_u8; 32]);

    assert_upgrade_rejected(
        &server,
        None,
        Some(WSS_SUBPROTOCOL),
        None,
        401,
        Some("AUTHENTICATION_FAILED"),
    )
    .await;
    assert_upgrade_rejected(
        &server,
        Some(&wrong_token),
        Some(WSS_SUBPROTOCOL),
        None,
        401,
        Some("AUTHENTICATION_FAILED"),
    )
    .await;
    assert_upgrade_rejected(
        &server,
        None,
        Some(WSS_SUBPROTOCOL),
        Some(server.operator().cookie.as_str()),
        401,
        Some("AUTHENTICATION_FAILED"),
    )
    .await;
    assert_upgrade_rejected(
        &server,
        Some(&token),
        None,
        None,
        400,
        Some("PROTOCOL_VERSION_UNSUPPORTED"),
    )
    .await;
    assert_upgrade_rejected(
        &server,
        Some(&token),
        Some("wrong.v1"),
        None,
        400,
        Some("PROTOCOL_VERSION_UNSUPPORTED"),
    )
    .await;

    let (mut socket, _epoch) = connect_and_hello(&server, &token, hardware_id).await;
    let device_id = server.only_device_id().await;
    server.revoke_device(&device_id).await;
    expect_server_drain_and_close(&mut socket).await;
    assert_upgrade_rejected(
        &server,
        Some(&token),
        Some(WSS_SUBPROTOCOL),
        None,
        401,
        Some("AUTHENTICATION_FAILED"),
    )
    .await;
    server.shutdown().await;
}

/// A connection that authenticated but has not sent its hello yet must still be reachable by
/// revocation: registering only after the hello exchange would leave a window in which a
/// revoked token keeps a live socket.
#[tokio::test]
async fn revocation_before_client_hello_still_evicts_the_authenticated_connection() {
    let server = TestServer::start().await;
    server.open_window().await;
    let hardware_id = machine_id(b"pre-hello-revoke");
    let client = server.client("pre-hello-revoke", hardware_id);
    client.enroll().await;
    let token = client.token();

    let mut socket = open_websocket(&server, &token).await;
    let device_id = server.only_device_id().await;
    server.revoke_device(&device_id).await;

    expect_server_drain_and_close(&mut socket).await;
    server.shutdown().await;
}

#[tokio::test]
async fn hello_registry_protocol_and_frame_boundaries_are_exact() {
    let server = TestServer::start().await;
    server.open_window().await;
    let hardware_id = machine_id(b"session-protocol");
    let client = server.client("session-protocol", hardware_id);
    client.enroll().await;
    let token = client.token();

    let (mut first, first_epoch) = connect_and_hello(&server, &token, hardware_id).await;
    let (mut second, second_epoch) = connect_and_hello(&server, &token, hardware_id).await;
    assert!(second_epoch > first_epoch);
    expect_server_drain_and_close(&mut first).await;
    assert_ping_round_trip(&mut second).await;
    close_client(&mut second).await;

    let mut not_hello = open_websocket(&server, &token).await;
    send_envelope(&mut not_hello, heartbeat_envelope()).await;
    expect_protocol_error(&mut not_hello, "PROTOCOL_INVALID_ENVELOPE").await;

    let mut wrong_version = open_websocket(&server, &token).await;
    send_envelope(&mut wrong_version, client_hello(hardware_id, 2)).await;
    expect_protocol_error(&mut wrong_version, "PROTOCOL_VERSION_UNSUPPORTED").await;

    let mut mismatch = open_websocket(&server, &token).await;
    send_envelope(
        &mut mismatch,
        client_hello(machine_id(b"different-hardware"), 1),
    )
    .await;
    expect_protocol_error(&mut mismatch, "PROTOCOL_INVALID_ENVELOPE").await;

    let (mut observed, _epoch) = connect_and_hello(&server, &token, hardware_id).await;
    send_envelope(&mut observed, observed_envelope()).await;
    expect_protocol_error(&mut observed, "PROTOCOL_INVALID_ENVELOPE").await;

    let (mut oversized, _epoch) = connect_and_hello(&server, &token, hardware_id).await;
    require_ok(
        oversized
            .send(WebSocketMessage::Binary(
                vec![0_u8; WSS_MAX_FRAME_BYTES + 1].into(),
            ))
            .await,
        "oversized client frame must be written",
    );
    expect_closed(&mut oversized).await;
    server.shutdown().await;
}

#[tokio::test]
async fn replacement_claim_evicts_old_connection_and_audits_true_while_first_issue_audits_false() {
    let server = TestServer::start().await;
    server.open_window().await;
    let hardware_id = machine_id(b"replacement-audit");
    let original = server.client("replacement-original", hardware_id);
    original.enroll().await;
    assert_eq!(server.issuance_eviction_flags(), vec![false]);
    let original_token = original.token();
    let (mut old_connection, _epoch) =
        connect_and_hello(&server, &original_token, hardware_id).await;

    let replacement = server.client("replacement-new", hardware_id);
    let pending = require_ok(
        replacement.client.step().await,
        "replacement request must become pending",
    );
    let request_id = pending_request_id(pending);
    server.approve_request(request_id).await;
    replacement.enroll().await;

    expect_server_drain_and_close(&mut old_connection).await;
    assert_ne!(replacement.token(), original_token);
    assert_eq!(server.issuance_eviction_flags(), vec![false, true]);
    server.shutdown().await;
}

#[tokio::test]
async fn failed_authentication_rate_limit_returns_transport_429_after_ten_failures() {
    let server = TestServer::start().await;
    let wrong_token = URL_SAFE_NO_PAD.encode([0x5a_u8; 32]);
    for _ in 0..10 {
        assert_upgrade_rejected(
            &server,
            Some(&wrong_token),
            Some(WSS_SUBPROTOCOL),
            None,
            401,
            Some("AUTHENTICATION_FAILED"),
        )
        .await;
    }
    assert_upgrade_rejected(
        &server,
        Some(&wrong_token),
        Some(WSS_SUBPROTOCOL),
        None,
        429,
        None,
    )
    .await;
    server.shutdown().await;
}

async fn assert_upgrade_rejected(
    server: &TestServer,
    token: Option<&str>,
    subprotocol: Option<&str>,
    cookie: Option<&str>,
    expected_status: u16,
    expected_code: Option<&str>,
) {
    let error = match server.websocket_attempt(token, subprotocol, cookie).await {
        Err(error) => error,
        Ok((socket, _response)) => {
            drop(socket);
            panic!("WebSocket upgrade must be rejected");
        }
    };
    let WebSocketError::Http(response) = error else {
        panic!("WebSocket rejection must carry the HTTP response");
    };
    assert_eq!(response.status().as_u16(), expected_status);
    assert!(response.headers().contains_key("x-correlation-id"));
    if let Some(expected_code) = expected_code {
        let body = response
            .body()
            .as_deref()
            .unwrap_or_else(|| panic!("stable HTTP error body must be present"));
        let value: Value = require_ok(serde_json::from_slice(body), "HTTP error body must be JSON");
        assert_eq!(
            value.get("status").and_then(Value::as_u64),
            Some(u64::from(expected_status))
        );
        assert_eq!(
            value.get("code").and_then(Value::as_str),
            Some(expected_code)
        );
        let body_correlation = value
            .get("correlation_id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("HTTP error correlation ID must be present"));
        let header_correlation = response
            .headers()
            .get("x-correlation-id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_else(|| panic!("correlation header must be text"));
        assert_eq!(body_correlation, header_correlation);
    } else {
        assert!(response.body().as_deref().is_none_or(<[u8]>::is_empty));
        assert!(!response.headers().contains_key(ws_header::CONTENT_TYPE));
    }
}

async fn open_websocket(server: &TestServer, token: &str) -> TestWebSocket {
    let (socket, response) = require_ok(
        server
            .websocket_attempt(Some(token), Some(WSS_SUBPROTOCOL), None)
            .await,
        "authenticated WebSocket upgrade must succeed",
    );
    assert_eq!(response.status().as_u16(), 101);
    assert_eq!(
        response
            .headers()
            .get(ws_header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok()),
        Some(WSS_SUBPROTOCOL)
    );
    socket
}

async fn connect_and_hello(
    server: &TestServer,
    token: &str,
    hardware_id: Uuid,
) -> (TestWebSocket, u64) {
    let mut socket = open_websocket(server, token).await;
    let before = unix_time_millis();
    send_envelope(&mut socket, client_hello(hardware_id, 1)).await;
    let envelope = receive_envelope(&mut socket).await;
    let after = unix_time_millis();
    let Some(control_envelope::Body::ServerHello(hello)) = envelope.body else {
        panic!("ClientHello must receive ServerHello");
    };
    assert_eq!(hello.wire_version, 1);
    assert!(hello.connection_epoch > 0);
    assert_eq!(hello.heartbeat_interval_ms, 20_000);
    assert_eq!(hello.idle_timeout_ms, 60_000);
    assert_eq!(hello.max_frame_bytes, 65_536);
    assert_eq!(hello.max_bulk_bytes, 1_048_576);
    assert!(hello.server_time_unix_ms >= before && hello.server_time_unix_ms <= after);
    assert_eq!(hello.terminal_result_resume_cursor, 0);
    assert!(hello.capabilities.is_empty());
    (socket, hello.connection_epoch)
}

async fn send_envelope(socket: &mut TestWebSocket, envelope: ControlEnvelope) {
    require_ok(
        socket
            .send(WebSocketMessage::Binary(envelope.encode_to_vec().into()))
            .await,
        "control envelope must be sent",
    );
}

async fn receive_envelope(socket: &mut TestWebSocket) -> ControlEnvelope {
    loop {
        let message = require_ok(
            timeout(WSS_EVENT_TIMEOUT, socket.next()).await,
            "control envelope must arrive within the bounded wait",
        );
        let message = require_ok(message.ok_or(()), "WebSocket must remain open");
        match require_ok(message, "WebSocket message must be readable") {
            WebSocketMessage::Binary(bytes) => {
                return require_ok(
                    ControlEnvelope::decode(bytes),
                    "binary frame must decode as a control envelope",
                );
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

async fn expect_protocol_error(socket: &mut TestWebSocket, expected_code: &str) {
    let envelope = receive_envelope(socket).await;
    let Some(control_envelope::Body::ProtocolError(error)) = envelope.body else {
        panic!("protocol rejection must send ProtocolError");
    };
    assert_eq!(error.stable_error_code, expected_code);
    expect_closed(socket).await;
}

async fn expect_server_drain_and_close(socket: &mut TestWebSocket) {
    let envelope = receive_envelope(socket).await;
    let Some(control_envelope::Body::ServerDrain(drain)) = envelope.body else {
        panic!("registry eviction must send ServerDrain");
    };
    assert_eq!(drain.reconnect_after_unix_ms, 0);
    expect_closed(socket).await;
}

async fn assert_ping_round_trip(socket: &mut TestWebSocket) {
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

async fn close_client(socket: &mut TestWebSocket) {
    let _close_result = socket.close(None).await;
    expect_closed(socket).await;
}

async fn expect_closed(socket: &mut TestWebSocket) {
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

fn client_hello(machine_hardware_id: Uuid, wire_version: u32) -> ControlEnvelope {
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
            terminal_result_cursor: 0,
        })),
    }
}

fn heartbeat_envelope() -> ControlEnvelope {
    ControlEnvelope {
        body: Some(control_envelope::Body::Heartbeat(Heartbeat {
            session_lock_state: SessionLockState::None as i32,
            ..Heartbeat::default()
        })),
    }
}

fn observed_envelope() -> ControlEnvelope {
    ControlEnvelope {
        body: Some(control_envelope::Body::ObservedState(
            ObservedStateSnapshot {
                state_apply_status: StateApplyStatus::Idle as i32,
                secret_state: SecretState::Absent as i32,
                gateway_state: GatewayState::Absent as i32,
                session_state: SessionState::None as i32,
                session_lock_state: SessionLockState::None as i32,
                home_state: HomeState::Unmounted as i32,
                ..ObservedStateSnapshot::default()
            },
        )),
    }
}

fn pending_request_id(step: EnrollmentStep) -> Uuid {
    match step {
        EnrollmentStep::Waiting(EnrollmentWaitState::ApprovalPending {
            enrollment_request_id,
        }) => enrollment_request_id,
        EnrollmentStep::Enrolled
        | EnrollmentStep::Rejected
        | EnrollmentStep::Waiting(
            EnrollmentWaitState::ProvisioningWindowClosed
            | EnrollmentWaitState::NetworkUnavailable
            | EnrollmentWaitState::ServerUnavailable,
        ) => panic!("replacement Enrollment step must be approval-pending"),
    }
}

fn install_server_pki(directory: &Path) -> ServerPki {
    let private_directory = directory.join("server-keys");
    require_ok(
        fs::create_dir(&private_directory),
        "server private directory must be created",
    );
    require_ok(
        fs::set_permissions(&private_directory, fs::Permissions::from_mode(0o700)),
        "server private directory mode must be set",
    );

    let (control_params, control_key, control_certificate_der) = make_ca();
    let control_issuer = Issuer::new(control_params, control_key);
    let server_key = require_ok(
        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256),
        "server TLS key must be generated",
    );
    let mut server_params = require_ok(
        CertificateParams::new(vec![LOCALHOST.to_string()]),
        "server TLS parameters must be created",
    );
    server_params.is_ca = IsCa::ExplicitNoCa;
    server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server_params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    server_params.not_after = rcgen::date_time_ymd(4090, 1, 1);
    let server_certificate = require_ok(
        server_params.signed_by(&server_key, &control_issuer),
        "server TLS leaf must be signed",
    );
    let server_certificate_path = private_directory.join("server-tls-leaf.der");
    let server_key_path = private_directory.join("server-tls-key.pk8");
    install_private(&server_certificate_path, server_certificate.der().as_ref());
    let mut server_key_der = server_key.serialize_der();
    install_private(&server_key_path, &server_key_der);
    server_key_der.zeroize();

    let (_origin_params, origin_key, origin_certificate_der) = make_ca();
    install_private(
        &private_directory.join("origin-ca.der"),
        &origin_certificate_der,
    );
    let mut origin_key_der = origin_key.serialize_der();
    install_private(
        &private_directory.join("origin-ca-key.pk8"),
        &origin_key_der,
    );
    origin_key_der.zeroize();
    let control_root_path = directory.join("control-ca.crt");
    let origin_root_path = directory.join("local-origin-ca.crt");
    write_certificate_pem(&control_root_path, &control_certificate_der);
    write_certificate_pem(&origin_root_path, &origin_certificate_der);

    ServerPki {
        private_directory,
        server_certificate_path,
        server_key_path,
        control_root_path,
        origin_root_path,
        control_certificate_der,
    }
}

fn make_ca() -> (CertificateParams, KeyPair, Vec<u8>) {
    let key = require_ok(
        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256),
        "test CA key must be generated",
    );
    let mut params = require_ok(
        CertificateParams::new(Vec::<String>::new()),
        "test CA parameters must be created",
    );
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(4095, 1, 1);
    let certificate = require_ok(
        params.self_signed(&key),
        "test CA certificate must be signed",
    );
    (params, key, certificate.der().to_vec())
}

fn install_private(path: &Path, contents: &[u8]) {
    let mut file = require_ok(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path),
        "private fixture must be created",
    );
    require_ok(file.write_all(contents), "private fixture must be written");
}

fn write_certificate_pem(path: &Path, certificate_der: &[u8]) {
    let encoded = STANDARD.encode(certificate_der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in encoded.as_bytes().chunks(64) {
        let line = require_ok(
            std::str::from_utf8(line),
            "certificate base64 must be UTF-8",
        );
        pem.push_str(line);
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    require_ok(fs::write(path, pem), "certificate PEM must be written");
}

fn machine_id(label: &[u8]) -> Uuid {
    Uuid::new_v5(&TEST_NAMESPACE, label)
}

fn unix_time_millis() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
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
