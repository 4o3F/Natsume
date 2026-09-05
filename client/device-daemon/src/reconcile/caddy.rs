use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
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
            .and_then(|encoded| serde_json::from_slice::<CaddyModeArtifact>(&encoded).ok());
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
            sample_leaf(&self.gateway_hostname, &self.origin_root_path)
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
        format!(
            "{}\n{} {{\n\tbind 127.0.0.1 ::1\n\ttls {} {}\n\t@login path /login\n\thandle @login {{\n\t\treverse_proxy {} {{\n\t\t\theader_up X-DOMjudge-Login {}\n\t\t\theader_up X-DOMjudge-Pass {}\n\t\t}}\n\t}}\n\thandle {{\n\t\treverse_proxy {} {{\n\t\t\theader_up -X-DOMjudge-Login\n\t\t\theader_up -X-DOMjudge-Pass\n\t\t}}\n\t}}\n}}\n",
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
            "{{\n\tadmin {}|0660\n\tpersist_config off\n\tauto_https off\n\tgrace_period 10s\n}}\n",
            self.admin_address()
        )
    }

    fn admin_address(&self) -> String {
        format!("unix/{}", self.admin_socket_path.display())
    }
}

async fn sample_leaf(hostname: &str, root_path: &Path) -> Option<Vec<u8>> {
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
        let configuration = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(configuration));
        let stream = TcpStream::connect(("127.0.0.1", 443)).await.ok()?;
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
mod tests {
    use uuid::Uuid;

    use super::*;

    fn caddy() -> Caddy {
        Caddy {
            gateway_hostname: "contest.natsume.test".to_owned(),
            binary_path: PathBuf::from("/test/caddy"),
            admin_socket_path: PathBuf::from("/run/test/admin.sock"),
            configuration_path: PathBuf::from("/run/test/caddy.caddyfile"),
            mode_path: PathBuf::from("/run/test/caddy-mode.json"),
            origin_root_path: PathBuf::from("/etc/test/origin.crt"),
        }
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
}
