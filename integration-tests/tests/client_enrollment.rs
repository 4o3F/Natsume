use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use natsume_device_daemon::enrollment::{
    EnrollmentClient, EnrollmentError, EnrollmentPaths, EnrollmentStep, EnrollmentWaitState,
    enroll_until_parked,
};
use natsume_machine_identity::EvidenceQuality;
use natsume_server::{commands, config::ServerConfig};
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use reqwest::{StatusCode, redirect::Policy};
use serde_json::Value;
use tempfile::{TempDir, tempdir};
use tokio::{
    sync::oneshot,
    task::JoinHandle,
    time::{sleep, timeout},
};
use uuid::Uuid;
use zeroize::{Zeroize as _, Zeroizing};

const LOCALHOST: Ipv4Addr = Ipv4Addr::LOCALHOST;
const GATEWAY_HOSTNAME: &str = "gateway.contest.example";
const TEST_NAMESPACE: Uuid = Uuid::from_u128(0x1234_5678_1234_5678_9234_5678_1234_5678);
const SERVER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const OPERATOR_LOGIN: &str = "wp4-admin";
const OPERATOR_PASSWORD: &str = "wp4-operator-password";

use natsume_integration_tests::harness::bootstrap_operator;

struct TestServer {
    directory: TempDir,
    address: SocketAddr,
    control_certificate_der: Vec<u8>,
    origin_certificate_der: Vec<u8>,
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
    origin_certificate_der: Vec<u8>,
}

impl ClientFixture {
    fn gateway_key(&self) -> PathBuf {
        self.keys_directory.join("gateway-key.pk8")
    }

    fn gateway_leaf(&self) -> PathBuf {
        self.keys_directory.join("gateway-leaf.der")
    }

    fn gateway_chain(&self) -> PathBuf {
        self.keys_directory.join("gateway-chain.der")
    }

    fn device_token(&self) -> PathBuf {
        self.keys_directory.join("device-token")
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
            control_certificate_der: pki.control_certificate_der,
            origin_certificate_der: pki.origin_certificate_der,
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
        let url = format!("https://{}/api/v2/health", self.address);
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
                .timeout(Duration::from_secs(3))
                .build(),
            "control HTTP client must build",
        )
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://{}{path}", self.address)
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
        assert!(
            cookie.starts_with("__Secure-natsume_session="),
            "operator session cookie name must be exact"
        );
        let cookie = Zeroizing::new(cookie.to_owned());
        let body = require_ok(
            response.bytes().await,
            "operator login response must be readable",
        );
        let identity = require_ok(
            serde_json::from_slice::<Value>(&body),
            "operator login response must be JSON",
        );
        assert_eq!(identity.get("role").and_then(Value::as_str), Some("admin"));
        OperatorSession { http, cookie }
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
        let body = require_ok(
            response.bytes().await,
            "provisioning-window response must be readable",
        );
        let window = require_ok(
            serde_json::from_slice::<Value>(&body),
            "provisioning-window response must be JSON",
        );
        assert_eq!(window.get("state").and_then(Value::as_str), Some("open"));
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
        self.client_in_keys_directory(name, machine_hardware_id, keys_directory)
    }

    fn client_in_keys_directory(
        &self,
        name: &str,
        machine_hardware_id: Uuid,
        keys_directory: PathBuf,
    ) -> ClientFixture {
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

    async fn pending_request_id_if_present(&self, machine_hardware_id: Uuid) -> Option<Uuid> {
        let response = require_ok(
            self.operator_request(reqwest::Method::GET, "/api/v2/enrollment-requests")
                .send()
                .await,
            "Enrollment review list request must complete",
        );
        assert_eq!(response.status(), StatusCode::OK);
        let body = require_ok(
            response.bytes().await,
            "Enrollment review list must be readable",
        );
        let review = require_ok(
            serde_json::from_slice::<Value>(&body),
            "Enrollment review list must be JSON",
        );
        let items = review
            .as_array()
            .unwrap_or_else(|| panic!("Enrollment review list must be an array"));
        let machine_hardware_id = machine_hardware_id.to_string();
        let mut pending = items.iter().filter(|item| {
            item.get("machine_hardware_id").and_then(Value::as_str)
                == Some(machine_hardware_id.as_str())
                && item.get("state").and_then(Value::as_str) == Some("pending")
        });
        let item = pending.next()?;
        assert!(
            pending.next().is_none(),
            "hardware identity must have one pending request"
        );
        assert_eq!(
            item.get("resolution").and_then(Value::as_str),
            Some("replace_device_credentials")
        );
        let request_id = item
            .get("enrollment_request_id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("pending request ID must be present"));
        let parsed = require_ok(Uuid::parse_str(request_id), "pending request ID must parse");
        assert_eq!(parsed.to_string(), request_id);
        Some(parsed)
    }

    async fn pending_request_id(&self, machine_hardware_id: Uuid) -> Uuid {
        self.pending_request_id_if_present(machine_hardware_id)
            .await
            .unwrap_or_else(|| panic!("pending replacement must be listed"))
    }

    async fn decide_request(&self, request_id: Uuid, approve: bool) -> Uuid {
        let (action, expected_state) = if approve {
            ("approve", "approved")
        } else {
            ("reject", "rejected")
        };
        let path = format!("/api/v2/enrollment-requests/{request_id}/actions/{action}");
        let response = require_ok(
            self.operator_request(reqwest::Method::POST, &path)
                .send()
                .await,
            "Enrollment review action must complete",
        );
        assert_eq!(response.status(), StatusCode::OK);
        let body = require_ok(
            response.bytes().await,
            "Enrollment review action response must be readable",
        );
        let outcome = require_ok(
            serde_json::from_slice::<Value>(&body),
            "Enrollment review action response must be JSON",
        );
        assert_eq!(
            outcome.get("enrollment_request_id").and_then(Value::as_str),
            Some(request_id.to_string().as_str())
        );
        assert_eq!(
            outcome.get("state").and_then(Value::as_str),
            Some(expected_state)
        );
        request_id
    }

    async fn decide(&self, machine_hardware_id: Uuid, approve: bool) -> Uuid {
        let request_id = self.pending_request_id(machine_hardware_id).await;
        self.decide_request(request_id, approve).await
    }

    async fn decide_if_pending(&self, machine_hardware_id: Uuid, approve: bool) -> Option<Uuid> {
        let request_id = self
            .pending_request_id_if_present(machine_hardware_id)
            .await?;
        Some(self.decide_request(request_id, approve).await)
    }

    async fn raw_post(&self, client: &EnrollmentClient) -> Vec<u8> {
        let response = require_ok(
            self.control_http_client()
                .post(client.endpoint())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(client.request_body_json())
                .send()
                .await,
            "raw Enrollment POST must complete",
        );
        assert_eq!(response.status(), StatusCode::CREATED);
        require_ok(
            response.bytes().await.map(|body| body.to_vec()),
            "raw Enrollment response body must be read",
        )
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
        origin_certificate_der,
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
        ) => panic!("Enrollment step must be approval-pending"),
    }
}

fn assert_mode(path: &Path, expected: u32) {
    let metadata = require_ok(fs::metadata(path), "artifact metadata must be readable");
    assert_eq!(metadata.mode() & 0o777, expected);
}

fn assert_final_artifacts(client: &ClientFixture, origin_der: &[u8]) {
    assert_final_artifacts_in(&client.keys_directory, origin_der);
}

fn assert_final_artifacts_in(keys_directory: &Path, origin_der: &[u8]) {
    let gateway_leaf = keys_directory.join("gateway-leaf.der");
    let gateway_chain = keys_directory.join("gateway-chain.der");
    let device_token = keys_directory.join("device-token");
    let leaf = require_ok(fs::read(&gateway_leaf), "Gateway leaf must be readable");
    assert!(!leaf.is_empty());
    assert_eq!(
        require_ok(fs::read(&gateway_chain), "Gateway chain must be readable"),
        origin_der
    );
    let token = require_ok(fs::read(&device_token), "Device Token must be readable");
    assert_eq!(token.len(), 43);
    assert!(!token.contains(&b'\n'));
    assert_mode(&gateway_leaf, 0o640);
    assert_mode(&gateway_chain, 0o640);
    assert_mode(&device_token, 0o600);
}

fn assert_no_final_artifacts(client: &ClientFixture) {
    assert!(client.gateway_key().is_file(), "prepared key must remain");
    for path in [
        client.gateway_leaf(),
        client.gateway_chain(),
        client.device_token(),
    ] {
        assert!(!path.exists(), "finalization wrote {}", path.display());
    }
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
async fn open_window_create_issues_and_persists_pinned_artifacts_over_real_tls() {
    let server = TestServer::start().await;
    server.open_window().await;
    let client = server.client("create", machine_id(b"create"));

    let step = require_ok(
        client.client.step().await,
        "create Enrollment step must succeed",
    );

    assert_eq!(step, EnrollmentStep::Enrolled);
    assert_final_artifacts(&client, &server.origin_certificate_der);
    server.shutdown().await;
}

#[tokio::test]
async fn different_spki_replace_is_approve_then_claim_over_real_tls() {
    let server = TestServer::start().await;
    server.open_window().await;
    let hardware_id = machine_id(b"replace");
    let original = server.client("replace-original", hardware_id);
    assert_eq!(
        require_ok(
            original.client.step().await,
            "initial Enrollment must issue"
        ),
        EnrollmentStep::Enrolled
    );
    let replacement = server.client("replace-new", hardware_id);

    let request_id = pending_request_id(require_ok(
        replacement.client.step().await,
        "replacement Enrollment must become pending",
    ));
    let repeated_request_id = pending_request_id(require_ok(
        replacement.client.step().await,
        "replacement poll must remain pending",
    ));
    assert_eq!(request_id, repeated_request_id);
    let reviewed_request_id = server.decide(hardware_id, true).await;
    assert_eq!(request_id, reviewed_request_id);
    assert_eq!(
        require_ok(
            replacement.client.step().await,
            "approved replacement claim must issue"
        ),
        EnrollmentStep::Enrolled
    );
    assert_final_artifacts(&replacement, &server.origin_certificate_der);
    server.shutdown().await;
}

#[tokio::test]
async fn replacement_over_existing_artifacts_converges_via_enroll_until_parked() {
    let server = TestServer::start().await;
    server.open_window().await;
    let hardware_id = machine_id(b"park-replacement");
    let original = server.client("park-original", hardware_id);
    assert_eq!(
        require_ok(
            original.client.step().await,
            "initial Enrollment must issue"
        ),
        EnrollmentStep::Enrolled
    );
    let old_leaf = require_ok(
        fs::read(original.gateway_leaf()),
        "original Gateway leaf must be readable",
    );
    let old_token = require_ok(
        fs::read(original.device_token()),
        "original Device Token must be readable",
    );
    require_ok(
        fs::remove_file(original.gateway_key()),
        "original Gateway key must be removed",
    );
    let replacement = server.client_in_keys_directory(
        "park-replacement",
        hardware_id,
        original.keys_directory.clone(),
    );
    let keys_directory = replacement.keys_directory.clone();
    let enrollment_task =
        tokio::spawn(async move { enroll_until_parked(&replacement.client).await });

    let _reviewed_request_id = require_ok(
        timeout(Duration::from_secs(30), async {
            loop {
                if let Some(request_id) = server.decide_if_pending(hardware_id, true).await {
                    break request_id;
                }
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await,
        "replacement request must become pending and be approved",
    );
    let joined = require_ok(
        timeout(Duration::from_mins(1), enrollment_task).await,
        "enroll_until_parked must finish within 60 seconds",
    );
    let enrollment_result = require_ok(joined, "enroll_until_parked task must join");
    require_ok(
        enrollment_result,
        "enroll_until_parked must converge after approval",
    );

    assert_final_artifacts_in(&keys_directory, &server.origin_certificate_der);
    let new_leaf = require_ok(
        fs::read(keys_directory.join("gateway-leaf.der")),
        "replacement Gateway leaf must be readable",
    );
    let new_token = require_ok(
        fs::read(keys_directory.join("device-token")),
        "replacement Device Token must be readable",
    );
    assert_ne!(new_leaf, old_leaf);
    assert_ne!(new_token, old_token);
    server.shutdown().await;
}

#[tokio::test]
async fn lost_issue_response_self_heals_with_same_spki_over_real_tls() {
    let server = TestServer::start().await;
    server.open_window().await;
    let client = server.client("lost-issue-response", machine_id(b"lost-issue-response"));

    let _ = server.raw_post(&client.client).await;
    assert_no_final_artifacts(&client);

    let step = require_ok(
        client.client.step().await,
        "same-SPKI Enrollment retry must issue",
    );

    assert_eq!(step, EnrollmentStep::Enrolled);
    assert_final_artifacts(&client, &server.origin_certificate_der);
    server.shutdown().await;
}

#[tokio::test]
async fn rejected_request_has_typed_terminal_step_without_entering_infinite_park() {
    let server = TestServer::start().await;
    server.open_window().await;
    let hardware_id = machine_id(b"reject");
    let original = server.client("reject-original", hardware_id);
    assert_eq!(
        require_ok(
            original.client.step().await,
            "initial Enrollment must issue"
        ),
        EnrollmentStep::Enrolled
    );
    let rejected = server.client("reject-new", hardware_id);
    let request_id = pending_request_id(require_ok(
        rejected.client.step().await,
        "replacement Enrollment must become pending",
    ));
    let reviewed_request_id = server.decide(hardware_id, false).await;
    assert_eq!(request_id, reviewed_request_id);

    let step = require_ok(
        rejected.client.step().await,
        "rejected Enrollment must classify",
    );

    assert_eq!(step, EnrollmentStep::Rejected);
    assert_no_final_artifacts(&rejected);
    server.shutdown().await;
}

#[tokio::test]
async fn closed_window_has_typed_wait_state_over_real_tls() {
    let server = TestServer::start().await;
    let client = server.client("closed", machine_id(b"closed"));

    let step = require_ok(
        client.client.step().await,
        "closed-window Enrollment must classify",
    );

    assert_eq!(
        step,
        EnrollmentStep::Waiting(EnrollmentWaitState::ProvisioningWindowClosed)
    );
    assert_no_final_artifacts(&client);
    server.shutdown().await;
}

#[tokio::test]
async fn tampered_real_leaf_with_wrong_spki_causes_zero_finalization_writes() {
    let server = TestServer::start().await;
    server.open_window().await;
    let expected = server.client("tamper-expected", machine_id(b"tamper-expected"));
    let other = server.client("tamper-other", machine_id(b"tamper-other"));
    let expected_body = server.raw_post(&expected.client).await;
    let other_body = server.raw_post(&other.client).await;
    let mut tampered = require_ok(
        serde_json::from_slice::<Value>(&expected_body),
        "issued response must parse for tampering",
    );
    let other = require_ok(
        serde_json::from_slice::<Value>(&other_body),
        "other issued response must parse",
    );
    let wrong_leaf = other
        .get("gateway_leaf_der")
        .cloned()
        .unwrap_or_else(|| panic!("other issued response must contain a leaf"));
    let tampered_object = tampered
        .as_object_mut()
        .unwrap_or_else(|| panic!("issued response must be an object"));
    tampered_object.insert("gateway_leaf_der".to_owned(), wrong_leaf);
    let tampered_body = require_ok(
        serde_json::to_vec(&tampered),
        "tampered response must serialize",
    );

    let result = expected.client.finalize_issued_response(&tampered_body);

    assert!(matches!(result, Err(EnrollmentError::LeafSpkiMismatch)));
    assert_no_final_artifacts(&expected);
    server.shutdown().await;
}
