use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use natsume_device_protocol::is_valid_domjudge_username;
use reqwest::Client;
use rustls::{ClientConfig, RootCertStore};
use rustls_pki_types::{CertificateDer, ServerName, pem::PemObject as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::{
    net::TcpStream,
    process::Command,
    time::{Duration, timeout},
};
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::atomic_write::{WritePolicy, atomic_write};

use super::{
    SnapshotError, binding::ValidatedBindingContext, check_cancellation, gateway::GatewayMaterial,
};

const MODE_FORMAT_VERSION: u32 = 1;
const CADDY_OPERATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Non-secret description paired with the runtime Caddy configuration.
///
/// The description is trusted only after the Caddy admin API confirms that the candidate
/// configuration is the currently loaded configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "state", rename_all = "snake_case")]
pub(super) enum CaddyModeArtifact {
    Blocked {
        format_version: u32,
        credential_id: Option<String>,
    },
    Ready {
        format_version: u32,
        credential_id: String,
        domjudge_origin: String,
        binding: ValidatedBindingContext,
    },
}

/// One verified observation of the loaded Caddy mode and served Gateway leaf.
pub(super) struct CaddyObservation {
    pub(super) mode: Option<CaddyModeArtifact>,
    pub(super) gateway_leaf_sha256: Option<Vec<u8>>,
}

/// Concrete controller for the one packaged Caddy instance.
pub(super) struct Caddy {
    gateway_hostname: String,
    gateway_port: u16,
    binary_path: PathBuf,
    admin_socket_path: PathBuf,
    configuration_path: PathBuf,
    mode_path: PathBuf,
    origin_root_path: PathBuf,
}

impl Caddy {
    pub(super) fn production(gateway_hostname: String) -> Self {
        Self {
            gateway_hostname,
            gateway_port: 443,
            binary_path: PathBuf::from("/usr/lib/natsume/caddy"),
            admin_socket_path: PathBuf::from("/run/natsume/caddy-admin.sock"),
            configuration_path: PathBuf::from("/run/natsume/caddy.caddyfile"),
            mode_path: PathBuf::from("/run/natsume/caddy-mode.json"),
            origin_root_path: PathBuf::from("/etc/natsume/trust/local-origin-ca.crt"),
        }
    }

    pub(super) async fn current_blocked(
        &self,
        material: Option<&GatewayMaterial>,
    ) -> Option<CaddyObservation> {
        let mode = CaddyModeArtifact::Blocked {
            format_version: MODE_FORMAT_VERSION,
            credential_id: material.map(|material| material.credential_id.clone()),
        };
        self.current(&self.render_blocked(material), &mode).await
    }

    pub(super) async fn current_ready(
        &self,
        material: &GatewayMaterial,
        domjudge_origin: &str,
        binding: &ValidatedBindingContext,
        password: &str,
    ) -> Option<CaddyObservation> {
        let mode = CaddyModeArtifact::Ready {
            format_version: MODE_FORMAT_VERSION,
            credential_id: material.credential_id.clone(),
            domjudge_origin: domjudge_origin.to_owned(),
            binding: binding.clone(),
        };
        let configuration =
            Zeroizing::new(self.render_ready(material, domjudge_origin, binding, password));
        self.current(&configuration, &mode).await
    }

    pub(super) async fn ensure_blocked(
        &self,
        material: Option<&GatewayMaterial>,
        cancellation: &CancellationToken,
    ) -> Result<CaddyObservation, SnapshotError> {
        let mode = CaddyModeArtifact::Blocked {
            format_version: MODE_FORMAT_VERSION,
            credential_id: material.map(|material| material.credential_id.clone()),
        };
        self.ensure(&self.render_blocked(material), &mode, cancellation)
            .await
    }

    pub(super) async fn ensure_ready(
        &self,
        material: &GatewayMaterial,
        domjudge_origin: &str,
        binding: &ValidatedBindingContext,
        password: &str,
        cancellation: &CancellationToken,
    ) -> Result<CaddyObservation, SnapshotError> {
        let mode = CaddyModeArtifact::Ready {
            format_version: MODE_FORMAT_VERSION,
            credential_id: material.credential_id.clone(),
            domjudge_origin: domjudge_origin.to_owned(),
            binding: binding.clone(),
        };
        let configuration =
            Zeroizing::new(self.render_ready(material, domjudge_origin, binding, password));
        self.ensure(&configuration, &mode, cancellation).await
    }

    async fn ensure(
        &self,
        configuration: &str,
        mode: &CaddyModeArtifact,
        cancellation: &CancellationToken,
    ) -> Result<CaddyObservation, SnapshotError> {
        check_cancellation(cancellation)?;
        if let Some(observation) = self.current(configuration, mode).await {
            return Ok(observation);
        }
        self.load(configuration, mode, cancellation).await
    }

    async fn load(
        &self,
        configuration: &str,
        mode: &CaddyModeArtifact,
        cancellation: &CancellationToken,
    ) -> Result<CaddyObservation, SnapshotError> {
        check_cancellation(cancellation)?;
        self.invalidate_mode()?;
        atomic_write(
            &self.configuration_path,
            configuration.as_bytes(),
            0o600,
            WritePolicy::Replace,
        )
        .map_err(|_| SnapshotError::Caddy)?;

        check_cancellation(cancellation)?;
        self.run_caddy("validate").await?;
        check_cancellation(cancellation)?;
        let expected = self.adapt().await?;
        check_cancellation(cancellation)?;
        self.run_caddy("reload").await?;
        check_cancellation(cancellation)?;
        if self.loaded_configuration().await? != expected {
            return Err(SnapshotError::Caddy);
        }

        check_cancellation(cancellation)?;
        let encoded = serde_json::to_vec(mode).map_err(|_| SnapshotError::Caddy)?;
        atomic_write(&self.mode_path, &encoded, 0o600, WritePolicy::Replace)
            .map_err(|_| SnapshotError::Caddy)?;
        Ok(self.observation_for_mode(mode.clone()).await)
    }

    fn invalidate_mode(&self) -> Result<(), SnapshotError> {
        match fs::remove_file(&self.mode_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(SnapshotError::Caddy),
        }
    }

    async fn run_caddy(&self, operation: &str) -> Result<(), SnapshotError> {
        let mut command = Command::new(&self.binary_path);
        command
            .arg(operation)
            .arg("--config")
            .arg(&self.configuration_path)
            .arg("--adapter")
            .arg("caddyfile");
        if operation == "reload" {
            command.arg("--address").arg(self.admin_address());
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let status = timeout(CADDY_OPERATION_TIMEOUT, command.status())
            .await
            .map_err(|_| SnapshotError::Caddy)?
            .map_err(|_| SnapshotError::Caddy)?;
        status.success().then_some(()).ok_or(SnapshotError::Caddy)
    }

    async fn adapt(&self) -> Result<serde_json::Value, SnapshotError> {
        let mut command = Command::new(&self.binary_path);
        command
            .arg("adapt")
            .arg("--config")
            .arg(&self.configuration_path)
            .arg("--adapter")
            .arg("caddyfile")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let output = timeout(CADDY_OPERATION_TIMEOUT, command.output())
            .await
            .map_err(|_| SnapshotError::Caddy)?
            .map_err(|_| SnapshotError::Caddy)?;
        if !output.status.success() {
            return Err(SnapshotError::Caddy);
        }
        serde_json::from_slice(&output.stdout).map_err(|_| SnapshotError::Caddy)
    }

    async fn loaded_configuration(&self) -> Result<serde_json::Value, SnapshotError> {
        let client = Client::builder()
            .unix_socket(self.admin_socket_path.clone())
            .timeout(CADDY_OPERATION_TIMEOUT)
            .build()
            .map_err(|_| SnapshotError::Caddy)?;
        let response = client
            .get("http://localhost/config/")
            .send()
            .await
            .map_err(|_| SnapshotError::Caddy)?;
        if !response.status().is_success() {
            return Err(SnapshotError::Caddy);
        }
        response
            .json::<serde_json::Value>()
            .await
            .map_err(|_| SnapshotError::Caddy)
    }

    pub(super) async fn observe(&self) -> CaddyObservation {
        let mode = fs::read(&self.mode_path)
            .ok()
            .and_then(|encoded| serde_json::from_slice::<CaddyModeArtifact>(&encoded).ok())
            .filter(|mode| match mode {
                CaddyModeArtifact::Ready { binding, .. } => {
                    is_valid_domjudge_username(&binding.domjudge_username)
                }
                CaddyModeArtifact::Blocked { .. } => true,
            });
        let Some(mode) = mode else {
            return CaddyObservation {
                mode: None,
                gateway_leaf_sha256: None,
            };
        };
        let format_version = match &mode {
            CaddyModeArtifact::Blocked { format_version, .. }
            | CaddyModeArtifact::Ready { format_version, .. } => *format_version,
        };
        if format_version != MODE_FORMAT_VERSION {
            return CaddyObservation {
                mode: None,
                gateway_leaf_sha256: None,
            };
        }
        let verified = matches!(
            (self.adapt().await, self.loaded_configuration().await),
            (Ok(expected), Ok(loaded)) if expected == loaded
        );
        if !verified {
            return CaddyObservation {
                mode: None,
                gateway_leaf_sha256: None,
            };
        }
        self.observation_for_mode(mode).await
    }

    async fn current(
        &self,
        configuration: &str,
        mode: &CaddyModeArtifact,
    ) -> Option<CaddyObservation> {
        if !self.candidate_artifacts_match(configuration, mode) {
            return None;
        }
        let expected = self.adapt().await.ok()?;
        let loaded = self.loaded_configuration().await.ok()?;
        if expected != loaded {
            return None;
        }
        Some(self.observation_for_mode(mode.clone()).await)
    }

    fn candidate_artifacts_match(&self, configuration: &str, mode: &CaddyModeArtifact) -> bool {
        let Some(loaded_mode) = fs::read(&self.mode_path)
            .ok()
            .and_then(|encoded| serde_json::from_slice::<CaddyModeArtifact>(&encoded).ok())
        else {
            return false;
        };
        if loaded_mode != *mode {
            return false;
        }
        let Some(encoded_configuration) = fs::read(&self.configuration_path).ok() else {
            return false;
        };
        let encoded_configuration = Zeroizing::new(encoded_configuration);
        encoded_configuration.as_slice() == configuration.as_bytes()
    }

    async fn observation_for_mode(&self, mode: CaddyModeArtifact) -> CaddyObservation {
        let has_gateway = match &mode {
            CaddyModeArtifact::Blocked { credential_id, .. } => credential_id.is_some(),
            CaddyModeArtifact::Ready { .. } => true,
        };
        let gateway_leaf_sha256 = if has_gateway {
            sample_leaf(
                &self.gateway_hostname,
                &self.origin_root_path,
                self.gateway_port,
            )
            .await
            .map(|leaf| Sha256::digest(leaf).to_vec())
        } else {
            None
        };
        CaddyObservation {
            mode: Some(mode),
            gateway_leaf_sha256,
        }
    }

    fn render_blocked(&self, material: Option<&GatewayMaterial>) -> String {
        let mut configuration = self.global_options();
        if let Some(material) = material {
            let _ = write!(
                configuration,
                "\n{} {{\n\tbind 127.0.0.1 ::1\n\ttls {} {}\n\trespond \"Natsume Gateway is blocked\" 503\n}}\n",
                caddy_quote(&format!("https://{}", self.gateway_hostname)),
                caddy_quote_path(&material.certificate_path),
                caddy_quote_path(&material.private_key_path),
            );
        }
        configuration
    }

    fn render_ready(
        &self,
        material: &GatewayMaterial,
        domjudge_origin: &str,
        binding: &ValidatedBindingContext,
        password: &str,
    ) -> String {
        let password = Zeroizing::new(STANDARD.encode(password.as_bytes()));
        // The shared username alphabet excludes both Caddyfile environment
        // expansion and runtime placeholders; JSON quoting alone cannot do so.
        format!(
            r"{}
{} {{
	bind 127.0.0.1 ::1
	tls {} {}
	@login path /login
	handle @login {{
		reverse_proxy {} {{
			header_up X-DOMjudge-Login {}
			header_up X-DOMjudge-Pass {}
		}}
	}}
	handle {{
		reverse_proxy {} {{
			header_up -X-DOMjudge-Login
			header_up -X-DOMjudge-Pass
		}}
	}}
}}
",
            self.global_options(),
            caddy_quote(&format!("https://{}", self.gateway_hostname)),
            caddy_quote_path(&material.certificate_path),
            caddy_quote_path(&material.private_key_path),
            caddy_quote(domjudge_origin),
            caddy_quote(&binding.domjudge_username),
            caddy_quote(&password),
            caddy_quote(domjudge_origin),
        )
    }

    fn global_options(&self) -> String {
        format!(
            r"{{
	admin {}|0660
	persist_config off
	auto_https off
	grace_period 10s
}}
",
            self.admin_address()
        )
    }

    fn admin_address(&self) -> String {
        format!("unix/{}", self.admin_socket_path.display())
    }
}

async fn sample_leaf(hostname: &str, root_path: &Path, port: u16) -> Option<Vec<u8>> {
    timeout(CADDY_OPERATION_TIMEOUT, async {
        let encoded = fs::read(root_path).ok()?;
        let roots = CertificateDer::pem_slice_iter(&encoded)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        let [root] = roots.as_slice() else {
            return None;
        };
        let mut root_store = RootCertStore::empty();
        root_store.add(root.clone()).ok()?;
        let configuration =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .ok()?
                .with_root_certificates(root_store)
                .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(configuration));
        let stream = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        let server_name = ServerName::try_from(hostname.to_owned()).ok()?;
        let stream = connector.connect(server_name, stream).await.ok()?;
        stream
            .get_ref()
            .1
            .peer_certificates()?
            .first()
            .map(|certificate| certificate.as_ref().to_vec())
    })
    .await
    .ok()
    .flatten()
}

fn caddy_quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

fn caddy_quote_path(path: &std::path::Path) -> String {
    caddy_quote(&path.to_string_lossy())
}

#[cfg(test)]
pub(super) mod tests {
    use uuid::Uuid;

    use super::*;

    fn caddy() -> Caddy {
        Caddy {
            gateway_hostname: "contest.natsume.test".to_owned(),
            gateway_port: 443,
            binary_path: PathBuf::from("/test/caddy"),
            admin_socket_path: PathBuf::from("/run/test/admin.sock"),
            configuration_path: PathBuf::from("/run/test/caddy.caddyfile"),
            mode_path: PathBuf::from("/run/test/caddy-mode.json"),
            origin_root_path: PathBuf::from("/etc/test/origin.crt"),
        }
    }

    pub(in crate::reconcile) fn fixture(
        directory: &tempfile::TempDir,
    ) -> Result<(Caddy, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt as _;
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let mut caddy = caddy();
        caddy.binary_path = directory.path().join("caddy-fixture");
        caddy.admin_socket_path = directory.path().join("admin.sock");
        caddy.configuration_path = directory.path().join("caddyfile");
        caddy.mode_path = directory.path().join("caddy-mode.json");
        fs::write(&caddy.binary_path, "#!/bin/sh\nprintf '{}\\n'\n")?;
        fs::set_permissions(&caddy.binary_path, fs::Permissions::from_mode(0o700))?;
        let listener = tokio::net::UnixListener::bind(&caddy.admin_socket_path)?;
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request).await;
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await;
            }
        });
        Ok((caddy, task))
    }

    pub(in crate::reconcile) async fn serve_gateway(
        caddy: &mut Caddy,
        directory: &tempfile::TempDir,
        key: &rcgen::KeyPair,
    ) -> Result<(rcgen::Certificate, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
        let certificate = rcgen::CertificateParams::new(vec![caddy.gateway_hostname.clone()])?
            .self_signed(key)?;
        caddy.origin_root_path = directory.path().join("origin.crt");
        fs::write(
            &caddy.origin_root_path,
            format!(
                "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
                STANDARD.encode(certificate.der())
            ),
        )?;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        caddy.gateway_port = listener.local_addr()?.port();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.der().clone()],
            rustls_pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )?;
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let _ = acceptor.accept(stream).await;
            }
        });
        Ok((certificate, task))
    }

    fn material() -> GatewayMaterial {
        GatewayMaterial {
            credential_id: Uuid::now_v7().hyphenated().to_string(),
            certificate_path: PathBuf::from("/var/lib/test/fullchain.pem"),
            private_key_path: PathBuf::from("/var/lib/test/key.pem"),
            leaf_sha256: Sha256::digest(b"leaf").to_vec(),
        }
    }

    #[test]
    fn admin_socket_is_group_writable_and_reload_uses_its_plain_address() {
        let caddy = caddy();

        assert_eq!(caddy.admin_address(), "unix//run/test/admin.sock");
        assert!(
            caddy
                .global_options()
                .contains("admin unix//run/test/admin.sock|0660")
        );
    }

    #[test]
    fn candidate_load_invalidates_previous_mode_first() {
        let directory = tempfile::TempDir::new()
            .unwrap_or_else(|error| panic!("test directory must be created: {error}"));
        let mut caddy = caddy();
        caddy.mode_path = directory.path().join("caddy-mode.json");
        fs::write(&caddy.mode_path, b"old-mode")
            .unwrap_or_else(|error| panic!("mode fixture must be written: {error}"));

        caddy
            .invalidate_mode()
            .unwrap_or_else(|error| panic!("mode invalidation must succeed: {error}"));

        assert!(!caddy.mode_path.exists());
    }

    #[test]
    fn exact_candidate_artifacts_include_the_secret_configuration() {
        let directory = tempfile::TempDir::new()
            .unwrap_or_else(|error| panic!("test directory must be created: {error}"));
        let mut caddy = caddy();
        caddy.configuration_path = directory.path().join("caddy.caddyfile");
        caddy.mode_path = directory.path().join("caddy-mode.json");
        let material = material();
        let binding = binding();
        let mode = CaddyModeArtifact::Ready {
            format_version: MODE_FORMAT_VERSION,
            credential_id: material.credential_id.clone(),
            domjudge_origin: "https://judge.example".to_owned(),
            binding: binding.clone(),
        };
        let configuration = caddy.render_ready(
            &material,
            "https://judge.example",
            &binding,
            "first-password",
        );
        fs::write(&caddy.configuration_path, &configuration)
            .unwrap_or_else(|error| panic!("configuration fixture must be written: {error}"));
        fs::write(
            &caddy.mode_path,
            serde_json::to_vec(&mode)
                .unwrap_or_else(|error| panic!("mode fixture must encode: {error}")),
        )
        .unwrap_or_else(|error| panic!("mode fixture must be written: {error}"));

        assert!(caddy.candidate_artifacts_match(&configuration, &mode));
        let changed_password = caddy.render_ready(
            &material,
            "https://judge.example",
            &binding,
            "changed-password",
        );
        assert!(!caddy.candidate_artifacts_match(&changed_password, &mode));
    }

    fn binding() -> ValidatedBindingContext {
        ValidatedBindingContext {
            binding_id: Uuid::now_v7().hyphenated().to_string(),
            account_id: Uuid::now_v7().hyphenated().to_string(),
            seat_code: "A-01".to_owned(),
            domjudge_username: "team-alpha".to_owned(),
            credential_revision: 1,
        }
    }

    #[test]
    fn blocked_configuration_has_no_upstream_or_credentials() {
        let rendered = caddy().render_blocked(Some(&material()));

        assert!(rendered.contains("\"https://contest.natsume.test\" {"));
        assert!(!rendered.contains("https://\"contest.natsume.test\""));
        assert!(rendered.contains("respond \"Natsume Gateway is blocked\" 503"));
        assert!(!rendered.contains("reverse_proxy"));
        assert!(!rendered.contains("X-DOMjudge"));
    }

    #[test]
    fn ready_configuration_injects_credentials_only_in_login_handler() {
        let password = "must-not-enter-mode-artifact";
        let binding = binding();
        let rendered =
            caddy().render_ready(&material(), "https://judge.example", &binding, password);

        assert_eq!(rendered.matches("header_up X-DOMjudge-Login").count(), 1);
        assert_eq!(rendered.matches("header_up X-DOMjudge-Pass").count(), 1);
        assert!(rendered.contains(&STANDARD.encode(password)));
        let login = rendered
            .split("handle @login")
            .nth(1)
            .unwrap_or_else(|| panic!("login handler must be present"));
        assert!(login.contains("X-DOMjudge-Login"));
        let default = rendered
            .split("\thandle {\n")
            .nth(1)
            .unwrap_or_else(|| panic!("default handler must be present"));
        assert!(default.contains("header_up -X-DOMjudge-Login"));
        assert!(default.contains("header_up -X-DOMjudge-Pass"));
    }

    #[test]
    fn non_secret_mode_artifact_does_not_contain_password_material() {
        let password = "must-not-enter-mode-artifact";
        let mode = CaddyModeArtifact::Ready {
            format_version: MODE_FORMAT_VERSION,
            credential_id: Uuid::now_v7().hyphenated().to_string(),
            domjudge_origin: "https://judge.example".to_owned(),
            binding: binding(),
        };
        let encoded = serde_json::to_string(&mode)
            .unwrap_or_else(|error| panic!("mode must encode: {error}"));

        assert!(!encoded.contains(password));
        assert!(!encoded.contains(&STANDARD.encode(password)));
    }

    #[tokio::test]
    async fn unsupported_username_in_a_persisted_mode_cannot_be_ready()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::TempDir::new()?;
        let (caddy, admin_task) = fixture(&directory)?;
        let mut binding = binding();
        binding.domjudge_username = "{http.request.host}".to_owned();
        let mode = CaddyModeArtifact::Ready {
            format_version: MODE_FORMAT_VERSION,
            credential_id: Uuid::now_v7().hyphenated().to_string(),
            domjudge_origin: "https://judge.example".to_owned(),
            binding,
        };
        fs::write(&caddy.mode_path, serde_json::to_vec(&mode)?)?;
        assert!(caddy.observe().await.mode.is_none());
        admin_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn gateway_leaf_sampling_does_not_require_a_global_crypto_provider()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(rustls::crypto::CryptoProvider::get_default().is_none());
        let directory = tempfile::TempDir::new()?;
        let mut caddy = caddy();
        let key = rcgen::KeyPair::generate()?;
        let (certificate, task) = serve_gateway(&mut caddy, &directory, &key).await?;
        let sampled = sample_leaf(
            &caddy.gateway_hostname,
            &caddy.origin_root_path,
            caddy.gateway_port,
        )
        .await;
        task.abort();
        assert_eq!(sampled.as_deref(), Some(certificate.der().as_ref()));
        assert!(rustls::crypto::CryptoProvider::get_default().is_none());
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires the packaged Caddy binary via CADDY_BIN and loopback sockets"]
    async fn packaged_caddy_preserves_literal_usernames() -> Result<(), Box<dyn std::error::Error>>
    {
        use tokio::net::TcpListener;

        let directory = tempfile::TempDir::new()?;
        let binary = PathBuf::from(std::env::var("CADDY_BIN")?);
        let expected_sha = include_str!("../../../../packaging/client/caddy.sha256")
            .split_whitespace()
            .next()
            .ok_or("missing packaged Caddy hash")?;
        assert_eq!(
            hex::encode(Sha256::digest(fs::read(&binary)?)),
            expected_sha
        );

        let certificate = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()])?;
        let mut material = material();
        material.certificate_path = directory.path().join("certificate.pem");
        material.private_key_path = directory.path().join("key.pem");
        fs::write(
            &material.certificate_path,
            format!(
                "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
                STANDARD.encode(certificate.cert.der())
            ),
        )?;
        fs::write(
            &material.private_key_path,
            format!(
                "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
                STANDARD.encode(certificate.signing_key.serialize_der())
            ),
        )?;
        let upstream_tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.cert.der().clone()],
            rustls_pki_types::PrivatePkcs8KeyDer::from(certificate.signing_key.serialize_der())
                .into(),
        )?;
        let upstream = TcpListener::bind(("127.0.0.1", 0)).await?;
        let origin = format!("https://{}", upstream.local_addr()?);
        let upstream_task = tokio::spawn(record_upstream_requests(upstream, upstream_tls));
        let client = Client::builder()
            .no_proxy()
            .add_root_certificate(reqwest::Certificate::from_der(certificate.cert.der())?)
            .timeout(Duration::from_secs(2))
            .build()?;
        let mut caddy = caddy();
        caddy.admin_socket_path = directory.path().join("admin.sock");
        caddy.configuration_path = directory.path().join("Caddyfile");
        for username in [
            "A".to_owned(),
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789".to_owned(),
            "_.@+-".to_owned(),
            "Team_1.test+contest@example.org".to_owned(),
            "u".repeat(64),
        ] {
            assert!(is_valid_domjudge_username(&username));
            let port_reservation = TcpListener::bind(("127.0.0.1", 0)).await?;
            caddy.gateway_hostname = port_reservation.local_addr()?.to_string();
            drop(port_reservation);
            let mut binding = binding();
            binding.domjudge_username = username.clone();
            fs::write(
                &caddy.configuration_path,
                caddy.render_ready(&material, &origin, &binding, "password-canary"),
            )?;
            let log_path = directory.path().join("caddy.log");
            let mut process = Command::new(&binary)
                .args(["run", "--adapter", "caddyfile", "--config"])
                .arg(&caddy.configuration_path)
                .env("SSL_CERT_FILE", &material.certificate_path)
                .env("SSL_CERT_DIR", directory.path())
                .env("XDG_DATA_HOME", directory.path())
                .env("XDG_CONFIG_HOME", directory.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(fs::File::create(&log_path)?)
                .kill_on_drop(true)
                .spawn()?;
            let url = format!("https://{}", caddy.gateway_hostname);
            let result = assert_forwarded_credentials(&client, &url, &username).await;
            assert!(
                result.is_ok(),
                "Caddy credential forwarding failed: {result:?}; {}",
                fs::read_to_string(&log_path)?
            );
            process.kill().await?;
        }
        upstream_task.abort();
        Ok(())
    }

    async fn assert_forwarded_credentials(
        client: &Client,
        url: &str,
        username: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(10), async {
            loop {
                if client
                    .get(format!("{url}/"))
                    .send()
                    .await
                    .is_ok_and(|response| response.status().is_success())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await?;
        for (path, is_login) in [
            ("/login", true),
            ("/login?next=%2F", true),
            ("/", false),
            ("/login/other", false),
        ] {
            let response = client
                .get(format!("{url}{path}"))
                .header("X-DOMjudge-Login", "spoofed-username")
                .header("X-DOMjudge-Pass", "spoofed-password")
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            let headers: Vec<_> = response
                .lines()
                .filter_map(|line| line.split_once(": "))
                .collect();
            for (name, expected) in [
                ("X-DOMjudge-Login", username.to_owned()),
                ("X-DOMjudge-Pass", STANDARD.encode("password-canary")),
            ] {
                let values: Vec<_> = headers
                    .iter()
                    .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| *value)
                    .collect();
                if is_login {
                    assert_eq!(values, [expected.as_str()], "{path} {name}");
                } else {
                    assert!(values.is_empty(), "credential forwarded to {path}");
                }
            }
        }
        Ok(())
    }

    async fn record_upstream_requests(
        listener: tokio::net::TcpListener,
        tls: rustls::ServerConfig,
    ) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut stream) = acceptor.accept(stream).await else {
                    return;
                };
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    let Ok(count) = stream.read(&mut buffer).await else {
                        return;
                    };
                    if count == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    request.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(&request).await;
                let _ = stream.shutdown().await;
            });
        }
    }
}
