use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use snafu::Snafu;
use tokio_tungstenite::Connector;
use uuid::Uuid;

use crate::{
    client_configuration::{
        CLIENT_CONFIG_PATH, CONTROL_ROOT_PATH, DEVICE_TOKEN_NAME, KEYS_DIRECTORY_PATH,
        read_endpoint, read_single_pem_certificate,
    },
    journal::Journal,
};

mod backoff;
mod connect;
mod fixture;
mod hello;
mod key;
mod manifest;
mod session;

#[cfg(feature = "fixture")]
use self::fixture::FixtureState;
use self::{
    backoff::ReconnectBackoff,
    connect::{AttemptError, build_tls_connector, control_url},
    hello::read_boot_id,
    session::log_session_outcome,
};

const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";
const CONTROL_KEY_NAME: &str = "control-key-1.pk8";
const CONTROL_MANIFEST_NAME: &str = "manifest.json";

/// Maximum reconnect delay used by the production loop, exposed only to integration fixtures.
#[cfg(feature = "fixture")]
pub const CONTROL_RECONNECT_MAX_SECONDS: u64 = backoff::CONTROL_RECONNECT_MAX_SECONDS;

#[derive(Clone)]
pub struct ControlPaths {
    client_config: PathBuf,
    control_root: PathBuf,
    device_token: PathBuf,
    journal_directory: PathBuf,
}

impl ControlPaths {
    #[must_use]
    pub fn production() -> Self {
        Self {
            client_config: PathBuf::from(CLIENT_CONFIG_PATH),
            control_root: PathBuf::from(CONTROL_ROOT_PATH),
            device_token: PathBuf::from(KEYS_DIRECTORY_PATH).join(DEVICE_TOKEN_NAME),
            journal_directory: PathBuf::from("/var/lib/natsume/journal"),
        }
    }

    #[must_use]
    pub fn new(
        client_config: PathBuf,
        control_root: PathBuf,
        device_token: PathBuf,
        journal_directory: PathBuf,
    ) -> Self {
        Self {
            client_config,
            control_root,
            device_token,
            journal_directory,
        }
    }
}

#[derive(Debug, Snafu)]
pub enum ControlError {
    #[snafu(display("the client control endpoint configuration is invalid"))]
    EndpointConfiguration,

    #[snafu(display("the control-plane trust root is invalid"))]
    ControlTrustRoot,

    #[snafu(display("the control TLS client could not be constructed"))]
    TlsConfiguration,

    #[snafu(display("the device control machine identity is invalid"))]
    MachineIdentity,

    #[snafu(display("the local boot identity is unavailable or invalid"))]
    BootIdentity,

    #[snafu(display("the device command journal could not be initialized"))]
    Journal,

    #[snafu(display("the Device control protocol or negotiated limits are unsupported"))]
    ProtocolUnsupported,
}

#[derive(Debug, Snafu)]
pub enum DormantControlIdentityError {
    #[snafu(display("the dormant Device control private key is absent or invalid"))]
    ControlKey,

    #[snafu(display("entropy for the dormant Device control private key is unavailable"))]
    ControlKeyEntropy,

    #[snafu(display("the dormant Device control private key could not be encoded"))]
    ControlKeyEncoding,

    #[snafu(display("the dormant Device control private key could not be persisted"))]
    ControlKeyPersistence,

    #[snafu(display("the dormant Device control manifest is absent or invalid"))]
    Manifest,

    #[snafu(display("the dormant Device control manifest could not be serialized"))]
    ManifestSerialization,

    #[snafu(display("the dormant Device control manifest could not be persisted"))]
    ManifestPersistence,
}

pub(crate) fn ensure_dormant_identity(
    control_directory: &Path,
    machine_hardware_id: Uuid,
) -> Result<(), DormantControlIdentityError> {
    if machine_hardware_id.get_version_num() != 5 {
        return Err(DormantControlIdentityError::Manifest);
    }

    let key_path = control_directory.join(CONTROL_KEY_NAME);
    let manifest_path = control_directory.join(CONTROL_MANIFEST_NAME);
    let control_key = key::load(&key_path)?;
    let stored_manifest = manifest::load(&manifest_path)?;
    reconcile_dormant_identity(
        &key_path,
        &manifest_path,
        machine_hardware_id,
        control_key,
        stored_manifest,
    )
}

fn reconcile_dormant_identity(
    key_path: &Path,
    manifest_path: &Path,
    machine_hardware_id: Uuid,
    control_key: Option<key::ControlKey>,
    stored_manifest: Option<manifest::ControlManifest>,
) -> Result<(), DormantControlIdentityError> {
    match (control_key, stored_manifest) {
        (None, Some(stored_manifest)) => key::load(key_path)?
            .ok_or(DormantControlIdentityError::ControlKey)
            .and_then(|control_key| {
                stored_manifest.validate(machine_hardware_id, control_key.public_key())
            }),
        (Some(control_key), Some(stored_manifest)) => {
            stored_manifest.validate(machine_hardware_id, control_key.public_key())
        }
        (Some(control_key), None) => manifest::create_or_validate(
            manifest_path,
            machine_hardware_id,
            control_key.public_key(),
        ),
        (None, None) => {
            let control_key = key::create_or_load(key_path)?;
            manifest::create_or_validate(
                manifest_path,
                machine_hardware_id,
                control_key.public_key(),
            )
        }
    }
}

pub struct ControlClient {
    endpoint: String,
    socket_address: SocketAddr,
    connector: Connector,
    machine_hardware_id: String,
    boot_id: String,
    device_token: PathBuf,
    journal: Journal,
    #[cfg(feature = "fixture")]
    fixture: FixtureState,
}

impl ControlClient {
    /// Prepares the fixed-trust WSS client after identity and Enrollment are complete.
    ///
    /// # Errors
    ///
    /// Returns a redacted fail-closed error for invalid endpoint, trust, identity, boot ID, or
    /// journal state.
    pub fn prepare(paths: ControlPaths, machine_hardware_id: Uuid) -> Result<Self, ControlError> {
        if machine_hardware_id.get_version_num() != 5 {
            return Err(ControlError::MachineIdentity);
        }
        let endpoint =
            read_endpoint(&paths.client_config).map_err(|_| ControlError::EndpointConfiguration)?;
        let control_certificate = read_single_pem_certificate(&paths.control_root)
            .map_err(|_| ControlError::ControlTrustRoot)?;
        let connector = build_tls_connector(control_certificate)?;
        let boot_id = read_boot_id(std::path::Path::new(BOOT_ID_PATH))?;
        let journal = Journal::open(paths.journal_directory).map_err(|_| ControlError::Journal)?;
        Ok(Self {
            endpoint: control_url(endpoint),
            socket_address: SocketAddr::new(endpoint.ip(), endpoint.port().get()),
            connector,
            machine_hardware_id: machine_hardware_id.to_string(),
            boot_id,
            device_token: paths.device_token,
            journal,
            #[cfg(feature = "fixture")]
            fixture: FixtureState::new(),
        })
    }

    /// Runs the reconnecting Device control channel.
    ///
    /// Unauthorized credentials remain installed and are retried only at maximum backoff. The
    /// loop returns only for a fail-closed protocol-version or negotiated-limit incompatibility.
    ///
    /// # Errors
    ///
    /// Returns `ControlError::ProtocolUnsupported` when the server requires an incompatible
    /// control protocol or advertises an unusable frame limit.
    pub async fn run(&self) -> Result<(), ControlError> {
        let mut backoff = ReconnectBackoff::new();
        loop {
            #[cfg(feature = "fixture")]
            self.fixture.record_connection_attempt();
            match self.connect_and_hello().await {
                Ok((socket, limits)) => {
                    #[cfg(feature = "fixture")]
                    self.fixture.record_successful_hello();
                    tracing::info!(
                        connection_epoch = limits.connection_epoch,
                        heartbeat_interval_ms = limits.heartbeat_interval_ms,
                        idle_timeout_ms = limits.idle_timeout.as_millis(),
                        max_frame_bytes = limits.max_frame_bytes,
                        max_bulk_bytes = limits.max_bulk_bytes,
                        capability_count = limits.capability_count,
                        "Device control hello completed"
                    );
                    let result = self.run_session(socket, limits).await;
                    backoff.record_session_progress(result.progress);
                    log_session_outcome(result.outcome);
                    let delay = backoff.take_delay().max(result.requested_delay());
                    tokio::time::sleep(delay).await;
                }
                Err(AttemptError::ProtocolUnsupported) => {
                    return Err(ControlError::ProtocolUnsupported);
                }
                Err(AttemptError::LocalCredential) => {
                    backoff.force_maximum();
                    tracing::error!(
                        control_state = "local_credential_invalid",
                        "Device control credential file or authorization header is unavailable"
                    );
                    tokio::time::sleep(backoff.take_delay()).await;
                }
                Err(AttemptError::Unauthorized) => {
                    backoff.force_maximum();
                    tracing::error!(
                        control_state = "unauthorized",
                        "Device control credential was rejected; local credentials are retained"
                    );
                    tokio::time::sleep(backoff.take_delay()).await;
                }
                Err(AttemptError::RateLimited) => {
                    tracing::warn!(
                        control_state = "rate_limited",
                        "Device control reconnect was rate limited"
                    );
                    tokio::time::sleep(backoff.take_delay()).await;
                }
                Err(AttemptError::Reconnect | AttemptError::Transport) => {
                    tracing::warn!(
                        control_state = "reconnecting",
                        "Device control connection is unavailable"
                    );
                    tokio::time::sleep(backoff.take_delay()).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
