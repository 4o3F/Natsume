use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write as _},
    net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use natsume_integration_tests::harness::bootstrap_operator;
use natsume_server::{commands, config::ServerConfig};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use reqwest::{StatusCode, redirect::Policy};
use tempfile::{TempDir, tempdir};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::oneshot,
    task::JoinHandle,
    time::{sleep, timeout},
};
use zeroize::Zeroize as _;

const LOCALHOST: Ipv4Addr = Ipv4Addr::LOCALHOST;
const GATEWAY_HOSTNAME: &str = "gateway.contest.example";
const OPERATOR_LOGIN: &str = "ingress-admin";
const OPERATOR_PASSWORD: &str = "ingress-operator-password";
const EXPECTED_HTTP_MAX_HEADER_COUNT: usize = 64;
const EXPECTED_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const SLOW_HEADER_MARGIN: Duration = Duration::from_secs(10);
const SERVER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

struct TestServer {
    _directory: TempDir,
    address: SocketAddr,
    control_certificate_der: Vec<u8>,
    control_root_path: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<bool>>,
}

struct ServerPki {
    private_directory: PathBuf,
    server_certificate_path: PathBuf,
    server_key_path: PathBuf,
    control_root_path: PathBuf,
    origin_root_path: PathBuf,
    control_certificate_der: Vec<u8>,
}

struct RawTlsConnection {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
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

        let server = Self {
            _directory: directory,
            address,
            control_certificate_der: pki.control_certificate_der,
            control_root_path: pki.control_root_path,
            shutdown: Some(shutdown),
            task: Some(task),
        };
        server.wait_until_ready().await;
        server
    }

    async fn wait_until_ready(&self) {
        let client = self.http_client();
        for _ in 0..100 {
            if let Ok(response) = client.get(self.api_url("/api/v2/health")).send().await
                && response.status() == StatusCode::OK
            {
                return;
            }
            assert!(
                !self.is_finished(),
                "real TLS server stopped before becoming ready"
            );
            sleep(Duration::from_millis(20)).await;
        }
        panic!("real TLS server did not become ready");
    }

    fn http_client(&self) -> reqwest::Client {
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
                .timeout(Duration::from_secs(4))
                .build(),
            "control HTTP client must build",
        )
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://{}{path}", self.address)
    }

    fn is_finished(&self) -> bool {
        self.task.as_ref().is_some_and(JoinHandle::is_finished)
    }

    fn begin_shutdown(&mut self) {
        let sender = self
            .shutdown
            .take()
            .unwrap_or_else(|| panic!("server shutdown sender must exist"));
        assert!(sender.send(()).is_ok(), "server shutdown signal must send");
    }

    async fn await_shutdown(mut self) {
        let task = self
            .task
            .take()
            .unwrap_or_else(|| panic!("server task must exist"));
        match timeout(SERVER_SHUTDOWN_TIMEOUT, task).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false) | Err(_)) | Err(_) => panic!("real TLS server must stop cleanly"),
        }
    }

    async fn shutdown(mut self) {
        self.begin_shutdown();
        self.await_shutdown().await;
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _send_result = sender.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl RawTlsConnection {
    fn connect(address: SocketAddr, control_root_path: &Path) -> Self {
        let mut command = Command::new("openssl");
        command
            .args([
                "s_client",
                "-quiet",
                "-ign_eof",
                "-verify_return_error",
                "-alpn",
                "http/1.1",
                "-connect",
            ])
            .arg(address.to_string())
            .arg("-CAfile")
            .arg(control_root_path)
            .arg("-verify_ip")
            .arg(LOCALHOST.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                panic!("openssl(1) is a required integration-test dependency")
            }
            Err(error) => {
                drop(error);
                panic!("raw TLS client process must start");
            }
        };
        let stdin = child
            .stdin
            .take()
            .unwrap_or_else(|| panic!("raw TLS client stdin must exist"));
        let stdout = child
            .stdout
            .take()
            .unwrap_or_else(|| panic!("raw TLS client stdout must exist"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    async fn write_all(&mut self, bytes: &[u8]) {
        require_ok(
            self.stdin.write_all(bytes).await,
            "raw TLS request bytes must be written",
        );
        require_ok(
            self.stdin.flush().await,
            "raw TLS request bytes must be flushed",
        );
    }

    async fn wait_for_exit(self, wait: Duration) -> Vec<u8> {
        let Self {
            mut child,
            stdin,
            mut stdout,
        } = self;
        let mut response = Vec::new();
        let completion = timeout(wait, async {
            tokio::join!(child.wait(), stdout.read_to_end(&mut response))
        })
        .await;

        match completion {
            Ok((Ok(_status), Ok(_bytes_read))) => {
                drop(stdin);
                response
            }
            Ok((status, read)) => {
                drop(status);
                drop(read);
                drop(stdin);
                panic!("raw TLS client process must finish cleanly");
            }
            Err(_) => {
                drop(stdin);
                let _kill_result = child.kill().await;
                panic!("raw TLS peer did not observe connection closure in time");
            }
        }
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
    server_params
        .distinguished_name
        .push(DnType::CommonName, "Natsume ingress test server");
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

fn require_ok<T, E>(result: Result<T, E>, message: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            drop(error);
            panic!("{message}");
        }
    }
}

#[tokio::test]
async fn rejects_excess_headers_and_recovers_on_a_fresh_connection() {
    let server = TestServer::start().await;
    let client = server.http_client();
    let mut request = client.get(server.api_url("/api/v2/health"));
    for index in 0..=EXPECTED_HTTP_MAX_HEADER_COUNT {
        request = request.header(format!("x-ingress-header-{index}"), "bounded");
    }
    let response = require_ok(
        request.send().await,
        "oversized-header request must receive a response",
    );
    assert_eq!(
        response.status(),
        StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE
    );

    let fresh_client = server.http_client();
    let health = require_ok(
        fresh_client
            .get(server.api_url("/api/v2/health"))
            .send()
            .await,
        "fresh health request must complete",
    );
    assert_eq!(health.status(), StatusCode::OK);
    server.shutdown().await;
}

#[tokio::test]
async fn closes_slow_partial_headers_after_the_configured_timeout() {
    let server = TestServer::start().await;
    let mut connection = RawTlsConnection::connect(server.address, &server.control_root_path);
    connection.write_all(b"GET /api/v2/he").await;
    let started = Instant::now();
    let _response = connection
        .wait_for_exit(EXPECTED_HEADER_READ_TIMEOUT + SLOW_HEADER_MARGIN)
        .await;
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_secs(1),
        "slow-header connection closed before the timeout could apply"
    );
    // Tighter than the wait_for_exit deadline above, so this assertion (not the raw-client
    // panic) is what rejects a drifted timeout constant.
    assert!(
        elapsed <= EXPECTED_HEADER_READ_TIMEOUT + Duration::from_secs(5),
        "slow-header connection remained open beyond the allowed margin"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn graceful_shutdown_drains_an_in_flight_request() {
    let mut server = TestServer::start().await;
    let body = format!(r#"{{"login_name":"{OPERATOR_LOGIN}","password":"{OPERATOR_PASSWORD}"}}"#);
    let headers = format!(
        "POST /api/v2/session HTTP/1.1\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        server.address,
        body.len(),
    );
    let mut connection = RawTlsConnection::connect(server.address, &server.control_root_path);
    connection.write_all(headers.as_bytes()).await;
    connection.write_all(&body.as_bytes()[..1]).await;
    sleep(Duration::from_millis(250)).await;

    server.begin_shutdown();
    sleep(Duration::from_millis(100)).await;
    assert!(
        !server.is_finished(),
        "server exited while an accepted request was still in flight"
    );
    connection.write_all(&body.as_bytes()[1..]).await;
    let response = connection.wait_for_exit(Duration::from_secs(5)).await;
    assert!(
        response
            .windows(b"HTTP/1.1 200 OK\r\n".len())
            .any(|window| window == b"HTTP/1.1 200 OK\r\n"),
        "in-flight request did not complete successfully during shutdown"
    );
    server.await_shutdown().await;
}
