use std::{
    fs,
    net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener},
    os::unix::fs::PermissionsExt as _,
    path::PathBuf,
    time::Duration,
};

use diesel::{Connection, QueryableByName, RunQueryDsl, sql_types::Text, sqlite::SqliteConnection};
use natsume_device_daemon::enrollment::{EnrollmentClient, EnrollmentPaths};
use natsume_machine_identity::EvidenceQuality;
use natsume_server::{commands, config::ServerConfig};
use reqwest::{StatusCode, redirect::Policy};
use serde_json::Value;
use tempfile::{TempDir, tempdir};
use tokio::{
    sync::{Mutex, oneshot},
    task::JoinHandle,
    time::{MissedTickBehavior, interval, sleep, timeout},
};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{bootstrap_operator, client::ClientFixture, pki::install_server_pki, require_ok};

pub(super) const LOCALHOST: Ipv4Addr = Ipv4Addr::LOCALHOST;
const SERVER_EVENT_TIMEOUT: Duration = Duration::from_secs(5);
const SERVER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const TEST_GATEWAY_HOSTNAME: &str = "gateway.contest.example";
static SERVER_BIND_HANDOFF: Mutex<()> = Mutex::const_new(());

pub struct TestServer {
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

pub struct OperatorSession {
    http: reqwest::Client,
    cookie: Zeroizing<String>,
}

#[derive(QueryableByName)]
struct TextValueRow {
    #[diesel(sql_type = Text)]
    value: String,
}

impl OperatorSession {
    #[must_use]
    pub fn cookie(&self) -> &str {
        self.cookie.as_str()
    }
}

impl TestServer {
    pub async fn start(
        bootstrap_driver: &str,
        operator_login: &str,
        operator_password: &str,
    ) -> Self {
        let directory = require_ok(tempdir(), "server test directory must be created");
        let pki = install_server_pki(directory.path());
        let database_path = directory.path().join("server.sqlite3");
        let vault_key_path = pki.private_directory.join("server-root.key");
        let site_path = directory.path().join("site.toml");
        require_ok(
            fs::write(
                &site_path,
                format!(
                    "gateway_hostname = \"{TEST_GATEWAY_HOSTNAME}\"\n\
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
            bootstrap_driver,
            &server_config_path,
            operator_login,
            operator_password,
        )
        .await;
        let config = require_ok(
            ServerConfig::load_from(&server_config_path),
            "server configuration must load",
        );
        // Parallel fixtures keep their own ephemeral port reserved through bootstrap. Serialize
        // only the drop-to-listen handoff so another fixture cannot claim this port in between.
        let bind_handoff = SERVER_BIND_HANDOFF.lock().await;
        drop(reservation);
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
        drop(bind_handoff);
        server.operator = Some(
            server
                .login_operator(operator_login, operator_password)
                .await,
        );
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
                .timeout(SERVER_EVENT_TIMEOUT)
                .build(),
            "control HTTP client must build",
        )
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://{}{path}", self.address)
    }

    async fn login_operator(
        &self,
        operator_login: &str,
        operator_password: &str,
    ) -> OperatorSession {
        let http = self.control_http_client();
        let response = require_ok(
            http.post(self.api_url("/api/v2/session"))
                .json(&serde_json::json!({
                    "login_name": operator_login,
                    "password": operator_password,
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

    /// Returns the established operator session.
    ///
    /// # Panics
    ///
    /// Panics if called before server startup completed operator login.
    #[must_use]
    pub fn operator(&self) -> &OperatorSession {
        self.operator
            .as_ref()
            .unwrap_or_else(|| panic!("operator session must be established"))
    }

    pub fn operator_request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let operator = self.operator();
        operator
            .http
            .request(method, self.api_url(path))
            .header(reqwest::header::COOKIE, operator.cookie.as_str())
    }

    /// Opens the fixture provisioning window.
    ///
    /// # Panics
    ///
    /// Panics when the real operator request fails or is rejected.
    pub async fn open_window(&self) {
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

    #[must_use]
    pub fn client(&self, name: &str, machine_hardware_id: Uuid) -> ClientFixture {
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
        let enrollment = require_ok(
            EnrollmentClient::prepare(
                EnrollmentPaths::new(
                    client_config.clone(),
                    self.control_root_path.clone(),
                    self.origin_root_path.clone(),
                    keys_directory.clone(),
                ),
                machine_hardware_id,
                EvidenceQuality::Medium,
                TEST_GATEWAY_HOSTNAME.to_owned(),
            ),
            "Enrollment client must prepare",
        );
        ClientFixture {
            enrollment,
            machine_hardware_id,
            client_config,
            control_root: self.control_root_path.clone(),
            keys_directory,
            journal_directory: self.directory.path().join(format!("client-{name}-journal")),
        }
    }

    /// Approves one pending fixture Enrollment request.
    ///
    /// # Panics
    ///
    /// Panics when the real operator request fails or is rejected.
    pub async fn approve_request(&self, request_id: Uuid) {
        let path = format!("/api/v2/enrollment-requests/{request_id}/actions/approve");
        let response = require_ok(
            self.operator_request(reqwest::Method::POST, &path)
                .send()
                .await,
            "Enrollment approval must complete",
        );
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// Returns the only Device ID in a single-device scenario.
    ///
    /// # Panics
    ///
    /// Panics when the request fails or the response does not contain exactly one Device.
    pub async fn only_device_id(&self) -> String {
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

    /// Opens a direct database observer for scenario-specific assertions.
    ///
    /// # Panics
    ///
    /// Panics when the fixture database path or connection is unavailable.
    #[must_use]
    pub fn observer(&self) -> SqliteConnection {
        let path = self
            .database_path
            .to_str()
            .unwrap_or_else(|| panic!("database path must be UTF-8"));
        require_ok(
            SqliteConnection::establish(path),
            "database observer must connect",
        )
    }

    #[must_use]
    pub fn device_id_for_hardware(&self, machine_hardware_id: Uuid) -> String {
        let mut connection = self.observer();
        require_ok(
            diesel::sql_query(
                "SELECT device_pk AS value FROM devices WHERE machine_hardware_id = ?",
            )
            .bind::<Text, _>(machine_hardware_id.to_string())
            .get_result::<TextValueRow>(&mut connection),
            "enrolled Device ID must be readable",
        )
        .value
    }

    /// Creates one Command through the real operator API.
    ///
    /// # Panics
    ///
    /// Panics when the request fails or the server does not create the Command.
    pub async fn put_command(&self, command_id: Uuid, device_id: &str, kind: &str, payload: Value) {
        let path = format!("/api/v2/commands/{command_id}");
        let response = require_ok(
            self.operator_request(reqwest::Method::PUT, &path)
                .json(&serde_json::json!({
                    "device_id": device_id,
                    "kind": kind,
                    "payload_version": 1,
                    "payload": payload,
                }))
                .send()
                .await,
            "Command PUT must complete",
        );
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[must_use]
    pub fn command_state(&self, command_id: Uuid) -> String {
        let mut connection = self.observer();
        require_ok(
            diesel::sql_query("SELECT state AS value FROM commands WHERE command_id = ?")
                .bind::<Text, _>(command_id.to_string())
                .get_result::<TextValueRow>(&mut connection),
            "Command state must be readable",
        )
        .value
    }

    /// Waits for externally persisted `SQLite` state to converge.
    ///
    /// The observer connection has no in-process notification boundary, so a bounded interval
    /// poll is required. Delay semantics prevent missed ticks from turning into a hot loop.
    pub async fn wait_for_command_state(&self, command_id: Uuid, expected: &str) {
        let completed = timeout(SERVER_EVENT_TIMEOUT, async {
            let mut poll = interval(Duration::from_millis(20));
            poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                poll.tick().await;
                if self.command_state(command_id) == expected {
                    return;
                }
            }
        })
        .await;
        require_ok(completed, "Command state must converge within the bound");
    }

    /// Revokes one fixture Device through the real operator API.
    ///
    /// # Panics
    ///
    /// Panics when the request fails or is rejected.
    pub async fn revoke_device(&self, device_id: &str) {
        let path = format!("/api/v2/devices/{device_id}/actions/revoke");
        let response = require_ok(
            self.operator_request(reqwest::Method::POST, &path)
                .send()
                .await,
            "Device revocation must complete",
        );
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[must_use]
    pub fn websocket_url(&self) -> String {
        format!("wss://{}/api/v2/device/control", self.address)
    }

    #[must_use]
    pub fn control_certificate_der(&self) -> &[u8] {
        &self.control_certificate_der
    }

    /// Stops the real server and verifies a clean task exit.
    ///
    /// # Panics
    ///
    /// Panics when shutdown signalling or the bounded task join fails.
    pub async fn shutdown(mut self) {
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
