use std::{
    fs, future,
    io::ErrorKind,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use natsume_machine_identity::EvidenceQuality;
use rcgen::{
    CertificateParams, DistinguishedName, KeyPair, PKCS_ECDSA_P256_SHA256, PublicKeyData as _,
};
use reqwest::{StatusCode, redirect::Policy};
use rustls::{
    RootCertStore,
    client::{WebPkiServerVerifier, danger::ServerCertVerifier as _},
};
use rustls_pki_types::{
    CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime, pem::PemObject as _,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use snafu::Snafu;
use uuid::Uuid;
use x509_parser::{
    certificate::X509Certificate, certification_request::X509CertificationRequest,
    prelude::FromDer as _,
};
use zeroize::{Zeroize as _, Zeroizing};

use crate::{
    CanonicalEndpoint,
    atomic_write::{WritePolicy, atomic_write},
    parse_endpoint,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const ENROLLMENT_POLL_INTERVAL_SECONDS: u64 = 5;
const ENROLLMENT_PATH: &str = "/api/v2/enrollment-requests";
const ENROLLMENT_PROTOCOL_VERSION: u32 = 1;
const GATEWAY_KEY_NAME: &str = "gateway-key.pk8";
const GATEWAY_LEAF_NAME: &str = "gateway-leaf.der";
const GATEWAY_CHAIN_NAME: &str = "gateway-chain.der";
const DEVICE_TOKEN_NAME: &str = "device-token";
const GATEWAY_ARTIFACT_MODE: u32 = 0o640;
const DEVICE_TOKEN_MODE: u32 = 0o600;
const DEVICE_TOKEN_LENGTH: usize = 43;

#[derive(Clone)]
pub struct EnrollmentPaths {
    client_config: PathBuf,
    control_root: PathBuf,
    local_origin_root: PathBuf,
    keys_directory: PathBuf,
}

impl EnrollmentPaths {
    #[must_use]
    pub fn production() -> Self {
        Self {
            client_config: PathBuf::from("/etc/natsume/config.toml"),
            control_root: PathBuf::from("/etc/natsume/trust/control-ca.crt"),
            local_origin_root: PathBuf::from("/etc/natsume/trust/local-origin-ca.crt"),
            keys_directory: PathBuf::from("/var/lib/natsume/keys"),
        }
    }

    #[must_use]
    pub fn new(
        client_config: PathBuf,
        control_root: PathBuf,
        local_origin_root: PathBuf,
        keys_directory: PathBuf,
    ) -> Self {
        Self {
            client_config,
            control_root,
            local_origin_root,
            keys_directory,
        }
    }

    fn gateway_key(&self) -> PathBuf {
        self.keys_directory.join(GATEWAY_KEY_NAME)
    }

    fn gateway_leaf(&self) -> PathBuf {
        self.keys_directory.join(GATEWAY_LEAF_NAME)
    }

    fn gateway_chain(&self) -> PathBuf {
        self.keys_directory.join(GATEWAY_CHAIN_NAME)
    }

    fn device_token(&self) -> PathBuf {
        self.keys_directory.join(DEVICE_TOKEN_NAME)
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum EnrollmentError {
    #[snafu(display("the client Enrollment endpoint configuration is invalid"))]
    EndpointConfiguration,

    #[snafu(display("the control-plane trust root is invalid"))]
    ControlTrustRoot,

    #[snafu(display("the local Origin CA trust root is invalid"))]
    OriginTrustRoot,

    #[snafu(display("the Enrollment HTTP client could not be constructed"))]
    HttpClient,

    #[snafu(display("the persisted Gateway private key is absent or invalid"))]
    GatewayKey,

    #[snafu(display("the Gateway private key could not be persisted"))]
    GatewayKeyPersistence,

    #[snafu(display("the Gateway certificate signing request could not be constructed"))]
    GatewayCsr,

    #[snafu(display("the Enrollment request could not be serialized"))]
    RequestSerialization,

    #[snafu(display("the Enrollment transport failed closed"))]
    Transport,

    #[snafu(display("the Enrollment response violated the wire contract"))]
    ResponseContract,

    #[snafu(display("the Enrollment request was rejected as invalid"))]
    RequestInvalid,

    #[snafu(display("the issued Gateway leaf certificate is invalid"))]
    InvalidLeaf,

    #[snafu(display("the issued Gateway leaf does not match the persisted private key"))]
    LeafSpkiMismatch,

    #[snafu(display("the issued Gateway certificate chain is invalid"))]
    InvalidChain,

    #[snafu(display("the issued Gateway leaf is outside its validity window"))]
    LeafValidityWindow,

    #[snafu(display("the issued Gateway leaf is not valid for the configured hostname"))]
    InvalidHostname,

    #[snafu(display("the issued Device Token has an invalid shape"))]
    InvalidToken,

    #[snafu(display("the issued Device credentials could not be persisted"))]
    CredentialPersistence,

    #[snafu(display("the enrolled local credential artifacts are absent or invalid"))]
    EnrolledArtifacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentWaitState {
    ApprovalPending { enrollment_request_id: Uuid },
    ProvisioningWindowClosed,
    NetworkUnavailable,
    ServerUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentStep {
    Enrolled,
    Rejected,
    Waiting(EnrollmentWaitState),
}

pub struct EnrollmentClient {
    http: reqwest::Client,
    endpoint: String,
    request_body: Vec<u8>,
    expected_spki_der: Vec<u8>,
    origin_certificate: CertificateDer<'static>,
    gateway_hostname: String,
    paths: EnrollmentPaths,
}

impl EnrollmentClient {
    /// Prepares one repeatable Enrollment request and persists or reuses its key first.
    ///
    /// # Errors
    ///
    /// Returns a redacted error when configuration, trust, key, or CSR preparation fails.
    pub fn prepare(
        paths: EnrollmentPaths,
        machine_hardware_id: Uuid,
        hardware_identity_quality: EvidenceQuality,
        gateway_hostname: String,
    ) -> Result<Self, EnrollmentError> {
        let endpoint = read_endpoint(&paths.client_config)?;
        let control_certificate =
            read_single_pem_certificate(&paths.control_root, TrustRootKind::Control)?;
        let origin_certificate =
            read_single_pem_certificate(&paths.local_origin_root, TrustRootKind::Origin)?;
        let http = build_http_client(&control_certificate)?;
        let key_pair = load_or_create_gateway_key(&paths.gateway_key())?;
        let (csr_der, expected_spki_der) = build_csr(&key_pair)?;
        let request = EnrollmentRequest {
            machine_hardware_id: machine_hardware_id.to_string(),
            hardware_identity_quality,
            gateway_csr_der: STANDARD.encode(csr_der),
            gateway_spki_sha256: hex::encode(Sha256::digest(&expected_spki_der)),
            client_version: env!("CARGO_PKG_VERSION"),
            protocol_version: ENROLLMENT_PROTOCOL_VERSION,
        };
        let request_body =
            serde_json::to_vec(&request).map_err(|_| EnrollmentError::RequestSerialization)?;
        Ok(Self {
            http,
            endpoint: enrollment_url(endpoint),
            request_body,
            expected_spki_der,
            origin_certificate,
            gateway_hostname,
            paths,
        })
    }

    /// Performs exactly one POST and returns its typed retry or terminal outcome.
    ///
    /// A `201` response is completely verified and persisted before `Enrolled` is returned.
    ///
    /// # Errors
    ///
    /// Returns a redacted fail-closed error for invalid local input, unexpected HTTP behavior,
    /// malformed responses, verification failures, or persistence failures.
    pub async fn step(&self) -> Result<EnrollmentStep, EnrollmentError> {
        let response = match self
            .http
            .post(&self.endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(self.request_body.clone())
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => return classify_transport_error(&error),
        };
        let status = response.status();
        if status.is_server_error() {
            return Ok(EnrollmentStep::Waiting(
                EnrollmentWaitState::ServerUnavailable,
            ));
        }
        let body = match response.bytes().await {
            Ok(body) => body,
            Err(error) => return classify_transport_error(&error),
        };
        match status {
            StatusCode::CREATED => {
                self.finalize_issued_response(&body)?;
                Ok(EnrollmentStep::Enrolled)
            }
            StatusCode::ACCEPTED => {
                let pending = serde_json::from_slice::<EnrollmentPendingResponse>(&body)
                    .map_err(|_| EnrollmentError::ResponseContract)?;
                let request_id = parse_canonical_uuid(&pending.enrollment_request_id, 7)
                    .ok_or(EnrollmentError::ResponseContract)?;
                match pending.state {
                    EnrollmentPendingState::Pending => Ok(EnrollmentStep::Waiting(
                        EnrollmentWaitState::ApprovalPending {
                            enrollment_request_id: request_id,
                        },
                    )),
                }
            }
            status if status.is_client_error() => classify_error_response(status, &body),
            _ => Err(EnrollmentError::ResponseContract),
        }
    }

    /// Integration-test fixture surface returning the exact JSON bytes reused by every POST;
    /// this is not daemon API.
    #[cfg(feature = "fixture")]
    #[must_use]
    pub fn request_body_json(&self) -> Vec<u8> {
        self.request_body.clone()
    }

    /// Integration-test fixture surface returning the configured HTTPS Enrollment endpoint;
    /// this is not daemon API.
    #[cfg(feature = "fixture")]
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Verifies and persists one issued response using the prepared key and trust material.
    ///
    /// # Errors
    ///
    /// Returns a redacted error before any finalization write when response verification fails.
    pub fn finalize_issued_response(&self, body: &[u8]) -> Result<(), EnrollmentError> {
        let issued = serde_json::from_slice::<EnrollmentIssuedResponse>(body)
            .map_err(|_| EnrollmentError::ResponseContract)?;
        let artifacts = verify_issued_response(
            issued,
            &self.expected_spki_der,
            &self.origin_certificate,
            &self.gateway_hostname,
        )?;
        persist_verified_artifacts(&self.paths, &artifacts)
    }
}

/// Repeats the single-step operation at the frozen interval until credentials are installed.
///
/// A rejected request is terminal and parks without returning so service restart policy cannot
/// hammer the server.
///
/// # Errors
///
/// Returns a redacted fail-closed error from a single Enrollment step.
pub async fn enroll_until_parked(client: &EnrollmentClient) -> Result<(), EnrollmentError> {
    let mut previous_wait_state = None;
    loop {
        match client.step().await? {
            EnrollmentStep::Enrolled => return Ok(()),
            EnrollmentStep::Rejected => {
                tracing::error!(
                    enrollment_state = "rejected",
                    "Enrollment request was rejected; waiting for staff intervention"
                );
                future::pending::<()>().await;
            }
            EnrollmentStep::Waiting(wait_state) => {
                if previous_wait_state != Some(wait_state) {
                    log_wait_state(wait_state);
                    previous_wait_state = Some(wait_state);
                }
                tokio::time::sleep(Duration::from_secs(ENROLLMENT_POLL_INTERVAL_SECONDS)).await;
            }
        }
    }
}

pub(crate) fn device_token_present(paths: &EnrollmentPaths) -> Result<bool, EnrollmentError> {
    match fs::symlink_metadata(paths.device_token()) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Ok(_) | Err(_) => Err(EnrollmentError::EnrolledArtifacts),
    }
}

pub(crate) fn validate_enrolled_artifacts(paths: &EnrollmentPaths) -> Result<(), EnrollmentError> {
    let key_bytes = Zeroizing::new(
        fs::read(paths.gateway_key()).map_err(|_| EnrollmentError::EnrolledArtifacts)?,
    );
    parse_gateway_key(&key_bytes).map_err(|_| EnrollmentError::EnrolledArtifacts)?;
    let leaf = fs::read(paths.gateway_leaf()).map_err(|_| EnrollmentError::EnrolledArtifacts)?;
    raw_certificate_spki_der(&leaf).map_err(|()| EnrollmentError::EnrolledArtifacts)?;
    Ok(())
}

#[derive(Serialize)]
struct EnrollmentRequest {
    machine_hardware_id: String,
    hardware_identity_quality: EvidenceQuality,
    gateway_csr_der: String,
    gateway_spki_sha256: String,
    client_version: &'static str,
    protocol_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentPendingResponse {
    enrollment_request_id: String,
    state: EnrollmentPendingState,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum EnrollmentPendingState {
    Pending,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentIssuedResponse {
    enrollment_request_id: String,
    state: EnrollmentIssuedState,
    device_id: String,
    device_token: Zeroizing<String>,
    gateway_leaf_der: String,
    gateway_chain_der: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum EnrollmentIssuedState {
    Issued,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorResponse {
    #[serde(rename = "title")]
    _title: String,
    status: u16,
    code: String,
    #[serde(rename = "correlation_id")]
    _correlation_id: Uuid,
}

struct VerifiedArtifacts {
    leaf_der: Vec<u8>,
    chain_der: Vec<u8>,
    device_token: Zeroizing<String>,
}

#[derive(Clone, Copy)]
enum TrustRootKind {
    Control,
    Origin,
}

#[derive(Deserialize)]
struct ClientConfig {
    server: ServerEndpointConfig,
}

#[derive(Deserialize)]
struct ServerEndpointConfig {
    ip: String,
    port: u16,
}

fn read_endpoint(path: &Path) -> Result<CanonicalEndpoint, EnrollmentError> {
    let encoded = fs::read_to_string(path).map_err(|_| EnrollmentError::EndpointConfiguration)?;
    let config = toml::from_str::<ClientConfig>(&encoded)
        .map_err(|_| EnrollmentError::EndpointConfiguration)?;
    parse_endpoint(&config.server.ip, &config.server.port.to_string())
        .map_err(|_| EnrollmentError::EndpointConfiguration)
}

fn enrollment_url(endpoint: CanonicalEndpoint) -> String {
    let authority = match endpoint.ip() {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    format!("https://{authority}:{}{ENROLLMENT_PATH}", endpoint.port())
}

fn read_single_pem_certificate(
    path: &Path,
    kind: TrustRootKind,
) -> Result<CertificateDer<'static>, EnrollmentError> {
    let invalid = || match kind {
        TrustRootKind::Control => EnrollmentError::ControlTrustRoot,
        TrustRootKind::Origin => EnrollmentError::OriginTrustRoot,
    };
    let encoded = fs::read(path).map_err(|_| invalid())?;
    let mut certificates = CertificateDer::pem_slice_iter(&encoded);
    let certificate = certificates.next().ok_or_else(invalid)?;
    let certificate = certificate.map_err(|_| invalid())?;
    if certificates.next().is_some() || raw_certificate_spki_der(certificate.as_ref()).is_err() {
        return Err(invalid());
    }
    Ok(certificate)
}

fn build_http_client(
    control_certificate: &CertificateDer<'static>,
) -> Result<reqwest::Client, EnrollmentError> {
    let root = reqwest::Certificate::from_der(control_certificate.as_ref())
        .map_err(|_| EnrollmentError::ControlTrustRoot)?;
    reqwest::Client::builder()
        .tls_backend_rustls()
        .tls_certs_only([root])
        .https_only(true)
        .redirect(Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| EnrollmentError::HttpClient)
}

fn load_or_create_gateway_key(path: &Path) -> Result<KeyPair, EnrollmentError> {
    match fs::read(path) {
        Ok(bytes) => {
            let bytes = Zeroizing::new(bytes);
            parse_gateway_key(&bytes)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
                .map_err(|_| EnrollmentError::GatewayKey)?;
            let mut encoded = key_pair.serialize_der();
            let result = atomic_write(
                path,
                &encoded,
                GATEWAY_ARTIFACT_MODE,
                WritePolicy::CreateOnly,
            );
            encoded.zeroize();
            result.map_err(|_| EnrollmentError::GatewayKeyPersistence)?;
            Ok(key_pair)
        }
        Err(_) => Err(EnrollmentError::GatewayKey),
    }
}

fn parse_gateway_key(encoded: &[u8]) -> Result<KeyPair, EnrollmentError> {
    KeyPair::from_pkcs8_der_and_sign_algo(
        &PrivatePkcs8KeyDer::from(encoded),
        &PKCS_ECDSA_P256_SHA256,
    )
    .map_err(|_| EnrollmentError::GatewayKey)
}

fn build_csr(key_pair: &KeyPair) -> Result<(Vec<u8>, Vec<u8>), EnrollmentError> {
    let mut params =
        CertificateParams::new(Vec::<String>::new()).map_err(|_| EnrollmentError::GatewayCsr)?;
    params.distinguished_name = DistinguishedName::new();
    let csr = params
        .serialize_request(key_pair)
        .map_err(|_| EnrollmentError::GatewayCsr)?;
    let csr_der = csr.der().to_vec();
    let spki_der = raw_csr_spki_der(&csr_der)
        .map_err(|()| EnrollmentError::GatewayCsr)?
        .to_vec();
    if spki_der != key_pair.subject_public_key_info() {
        return Err(EnrollmentError::GatewayCsr);
    }
    Ok((csr_der, spki_der))
}

fn raw_csr_spki_der(csr_der: &[u8]) -> Result<&[u8], ()> {
    let (remainder, csr) = X509CertificationRequest::from_der(csr_der).map_err(|_| ())?;
    if !remainder.is_empty() {
        return Err(());
    }
    Ok(csr.certification_request_info.subject_pki.raw)
}

fn raw_certificate_spki_der(certificate_der: &[u8]) -> Result<&[u8], ()> {
    let (remainder, certificate) = X509Certificate::from_der(certificate_der).map_err(|_| ())?;
    if !remainder.is_empty() {
        return Err(());
    }
    Ok(certificate.tbs_certificate.subject_pki.raw)
}

fn classify_transport_error(error: &reqwest::Error) -> Result<EnrollmentStep, EnrollmentError> {
    // Body/decode failures are mid-transfer connection losses (reqwest wraps them as
    // `Body`, whose source is not the connector, so `is_connect` stays false). The frozen
    // poll semantics treat every transient network failure as a wait state; the issued
    // response that may have been lost is recovered by the same-SPKI retry.
    if error.is_connect() || error.is_timeout() || error.is_body() || error.is_decode() {
        Ok(EnrollmentStep::Waiting(
            EnrollmentWaitState::NetworkUnavailable,
        ))
    } else {
        Err(EnrollmentError::Transport)
    }
}

fn classify_error_response(
    status: StatusCode,
    body: &[u8],
) -> Result<EnrollmentStep, EnrollmentError> {
    let response = serde_json::from_slice::<ErrorResponse>(body)
        .map_err(|_| EnrollmentError::ResponseContract)?;
    if response.status != status.as_u16() {
        return Err(EnrollmentError::ResponseContract);
    }
    match response.code.as_str() {
        "ENROLLMENT_REQUEST_REJECTED" => Ok(EnrollmentStep::Rejected),
        "PROVISIONING_WINDOW_CLOSED" => Ok(EnrollmentStep::Waiting(
            EnrollmentWaitState::ProvisioningWindowClosed,
        )),
        _ => Err(EnrollmentError::RequestInvalid),
    }
}

fn verify_issued_response(
    issued: EnrollmentIssuedResponse,
    expected_spki_der: &[u8],
    origin_certificate: &CertificateDer<'static>,
    gateway_hostname: &str,
) -> Result<VerifiedArtifacts, EnrollmentError> {
    let EnrollmentIssuedResponse {
        enrollment_request_id,
        state,
        device_id,
        device_token,
        gateway_leaf_der,
        gateway_chain_der,
    } = issued;
    if parse_canonical_uuid(&enrollment_request_id, 7).is_none()
        || parse_canonical_uuid(&device_id, 7).is_none()
    {
        return Err(EnrollmentError::ResponseContract);
    }
    match state {
        EnrollmentIssuedState::Issued => {}
    }

    let leaf_der = STANDARD
        .decode(gateway_leaf_der)
        .map_err(|_| EnrollmentError::InvalidLeaf)?;
    let [chain_der] = gateway_chain_der.as_slice() else {
        return Err(EnrollmentError::InvalidChain);
    };
    let chain_der = STANDARD
        .decode(chain_der)
        .map_err(|_| EnrollmentError::InvalidChain)?;
    let leaf_spki =
        raw_certificate_spki_der(&leaf_der).map_err(|()| EnrollmentError::InvalidLeaf)?;
    if leaf_spki != expected_spki_der {
        return Err(EnrollmentError::LeafSpkiMismatch);
    }
    if chain_der.as_slice() != origin_certificate.as_ref() {
        return Err(EnrollmentError::InvalidChain);
    }
    verify_server_certificate(&leaf_der, origin_certificate, gateway_hostname)?;
    if !valid_device_token(&device_token) {
        return Err(EnrollmentError::InvalidToken);
    }
    Ok(VerifiedArtifacts {
        leaf_der,
        chain_der,
        device_token,
    })
}

fn verify_server_certificate(
    leaf_der: &[u8],
    origin_certificate: &CertificateDer<'static>,
    gateway_hostname: &str,
) -> Result<(), EnrollmentError> {
    let mut roots = RootCertStore::empty();
    roots
        .add(origin_certificate.clone())
        .map_err(|_| EnrollmentError::InvalidChain)?;
    let verifier = WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .build()
    .map_err(|_| EnrollmentError::InvalidChain)?;
    let server_name = ServerName::try_from(gateway_hostname.to_owned())
        .map_err(|_| EnrollmentError::InvalidHostname)?;
    match verifier.verify_server_cert(
        &CertificateDer::from(leaf_der),
        &[],
        &server_name,
        &[],
        UnixTime::now(),
    ) {
        Ok(_) => Ok(()),
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::Expired
            | rustls::CertificateError::ExpiredContext { .. }
            | rustls::CertificateError::NotValidYet
            | rustls::CertificateError::NotValidYetContext { .. },
        )) => Err(EnrollmentError::LeafValidityWindow),
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::NotValidForName
            | rustls::CertificateError::NotValidForNameContext { .. },
        )) => Err(EnrollmentError::InvalidHostname),
        Err(_) => Err(EnrollmentError::InvalidChain),
    }
}

fn valid_device_token(token: &str) -> bool {
    token.len() == DEVICE_TOKEN_LENGTH
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        && token
            .as_bytes()
            .last()
            .is_some_and(|byte| b"AEIMQUYcgkosw048".contains(byte))
}

fn persist_verified_artifacts(
    paths: &EnrollmentPaths,
    artifacts: &VerifiedArtifacts,
) -> Result<(), EnrollmentError> {
    atomic_write(
        &paths.gateway_leaf(),
        &artifacts.leaf_der,
        GATEWAY_ARTIFACT_MODE,
        WritePolicy::Replace,
    )
    .map_err(|_| EnrollmentError::CredentialPersistence)?;
    atomic_write(
        &paths.gateway_chain(),
        &artifacts.chain_der,
        GATEWAY_ARTIFACT_MODE,
        WritePolicy::Replace,
    )
    .map_err(|_| EnrollmentError::CredentialPersistence)?;
    atomic_write(
        &paths.device_token(),
        artifacts.device_token.as_bytes(),
        DEVICE_TOKEN_MODE,
        WritePolicy::Replace,
    )
    .map_err(|_| EnrollmentError::CredentialPersistence)
}

fn parse_canonical_uuid(value: &str, version: usize) -> Option<Uuid> {
    Uuid::parse_str(value)
        .ok()
        .filter(|uuid| uuid.get_version_num() == version && uuid.hyphenated().to_string() == value)
}

fn log_wait_state(state: EnrollmentWaitState) {
    match state {
        EnrollmentWaitState::ApprovalPending { .. } => tracing::info!(
            startup_identity_state = "enrollment_pending",
            enrollment_wait_state = "approval_pending",
            "Enrollment is waiting for operator approval"
        ),
        EnrollmentWaitState::ProvisioningWindowClosed => tracing::info!(
            startup_identity_state = "enrollment_pending",
            enrollment_wait_state = "provisioning_window_closed",
            "Enrollment is waiting for the provisioning window"
        ),
        EnrollmentWaitState::NetworkUnavailable => tracing::warn!(
            startup_identity_state = "enrollment_pending",
            enrollment_wait_state = "network_unavailable",
            "Enrollment is waiting for the control server"
        ),
        EnrollmentWaitState::ServerUnavailable => tracing::warn!(
            startup_identity_state = "enrollment_pending",
            enrollment_wait_state = "server_unavailable",
            "Enrollment is waiting for the control server to recover"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{MetadataExt as _, symlink},
        path::Path,
    };

    use rcgen::{BasicConstraints, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyUsagePurpose};
    use serde_json::json;
    use tempfile::{TempDir, tempdir};

    use super::*;

    const GATEWAY_HOSTNAME: &str = "gateway.contest.example";
    const FIXTURE_SPKI_SHA256: &str =
        "745c9ef1008168dde92273abeccc149d502cf6d88f79041f8689275c929b54ee";
    const VALID_TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    fn require_tempdir() -> TempDir {
        match tempdir() {
            Ok(directory) => directory,
            Err(error) => panic!("test directory must be created: {error}"),
        }
    }

    fn require_key() -> KeyPair {
        match KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256) {
            Ok(key) => key,
            Err(error) => panic!("test key must be generated: {error}"),
        }
    }

    fn fixture_csr_der() -> Vec<u8> {
        match STANDARD.decode(include_str!("../tests/fixtures/gateway-csr.der.base64").trim()) {
            Ok(encoded) => encoded,
            Err(error) => panic!("public CSR fixture must decode: {error}"),
        }
    }

    fn fixture_paths(directory: &TempDir) -> EnrollmentPaths {
        let keys_directory = directory.path().join("keys");
        if let Err(error) = fs::create_dir(&keys_directory) {
            panic!("keys directory must be created: {error}");
        }
        EnrollmentPaths::new(
            directory.path().join("config.toml"),
            directory.path().join("control-ca.crt"),
            directory.path().join("local-origin-ca.crt"),
            keys_directory,
        )
    }

    fn require_origin() -> (Issuer<'static, KeyPair>, CertificateDer<'static>) {
        let key = require_key();
        let mut params = match CertificateParams::new(Vec::<String>::new()) {
            Ok(params) => params,
            Err(error) => panic!("Origin CA parameters must be created: {error}"),
        };
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
        ];
        let certificate = match params.self_signed(&key) {
            Ok(certificate) => certificate,
            Err(error) => panic!("Origin CA certificate must be created: {error}"),
        };
        let certificate_der = certificate.der().clone();
        (Issuer::new(params, key), certificate_der)
    }

    fn require_leaf(key: &KeyPair, issuer: &Issuer<'_, KeyPair>, hostname: &str) -> Vec<u8> {
        require_leaf_with_validity(key, issuer, hostname, 2020, 4090)
    }

    fn require_leaf_with_validity(
        key: &KeyPair,
        issuer: &Issuer<'_, KeyPair>,
        hostname: &str,
        not_before_year: i32,
        not_after_year: i32,
    ) -> Vec<u8> {
        let mut params = match CertificateParams::new(vec![hostname.to_owned()]) {
            Ok(params) => params,
            Err(error) => panic!("Gateway leaf parameters must be created: {error}"),
        };
        params.distinguished_name = DistinguishedName::new();
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.not_before = rcgen::date_time_ymd(not_before_year, 1, 1);
        params.not_after = rcgen::date_time_ymd(not_after_year, 1, 1);
        match params.signed_by(key, issuer) {
            Ok(certificate) => certificate.der().to_vec(),
            Err(error) => panic!("Gateway leaf must be signed: {error}"),
        }
    }

    fn issued_response(
        leaf_der: &[u8],
        chain_der: &[&[u8]],
        token: &str,
    ) -> EnrollmentIssuedResponse {
        let encoded = match serde_json::to_vec(&json!({
            "enrollment_request_id": Uuid::now_v7(),
            "state": "issued",
            "device_id": Uuid::now_v7(),
            "device_token": token,
            "gateway_leaf_der": STANDARD.encode(leaf_der),
            "gateway_chain_der": chain_der
                .iter()
                .map(|certificate| STANDARD.encode(certificate))
                .collect::<Vec<_>>(),
        })) {
            Ok(encoded) => encoded,
            Err(error) => panic!("issued response fixture must serialize: {error}"),
        };
        match serde_json::from_slice(&encoded) {
            Ok(response) => response,
            Err(error) => panic!("issued response fixture must deserialize: {error}"),
        }
    }

    fn finalize_fixture(
        paths: &EnrollmentPaths,
        issued: EnrollmentIssuedResponse,
        expected_spki_der: &[u8],
        origin_certificate: &CertificateDer<'static>,
        hostname: &str,
    ) -> Result<(), EnrollmentError> {
        let artifacts =
            verify_issued_response(issued, expected_spki_der, origin_certificate, hostname)?;
        persist_verified_artifacts(paths, &artifacts)
    }

    fn assert_no_finalization_writes(paths: &EnrollmentPaths) {
        for path in [
            paths.gateway_leaf(),
            paths.gateway_chain(),
            paths.device_token(),
        ] {
            assert!(
                !path.exists(),
                "verification failure wrote {}",
                path.display()
            );
        }
    }

    fn require_contents(path: &Path) -> Vec<u8> {
        match fs::read(path) {
            Ok(contents) => contents,
            Err(error) => panic!("artifact must be readable: {error}"),
        }
    }

    #[test]
    fn gateway_key_is_created_once_and_reused_with_mode_0640() {
        let directory = require_tempdir();
        let paths = fixture_paths(&directory);
        let key_path = paths.gateway_key();

        let created = match load_or_create_gateway_key(&key_path) {
            Ok(key) => key,
            Err(error) => panic!("Gateway key must be created: {error}"),
        };
        let first_contents = require_contents(&key_path);
        let reused = match load_or_create_gateway_key(&key_path) {
            Ok(key) => key,
            Err(error) => panic!("Gateway key must be reused: {error}"),
        };

        assert_eq!(
            created.subject_public_key_info(),
            reused.subject_public_key_info()
        );
        assert_eq!(require_contents(&key_path), first_contents);
        let metadata = match fs::metadata(&key_path) {
            Ok(metadata) => metadata,
            Err(error) => panic!("Gateway key metadata must be readable: {error}"),
        };
        assert_eq!(metadata.mode() & 0o777, GATEWAY_ARTIFACT_MODE);
    }

    #[test]
    fn corrupt_existing_gateway_key_is_never_replaced() {
        let directory = require_tempdir();
        let paths = fixture_paths(&directory);
        if let Err(error) = fs::write(paths.gateway_key(), b"not a PKCS#8 key") {
            panic!("corrupt key fixture must be written: {error}");
        }

        let result = load_or_create_gateway_key(&paths.gateway_key());

        assert!(matches!(result, Err(EnrollmentError::GatewayKey)));
        assert_eq!(require_contents(&paths.gateway_key()), b"not a PKCS#8 key");
    }

    #[test]
    fn token_symlink_fails_closed_as_enrolled_artifacts() {
        let directory = require_tempdir();
        let paths = fixture_paths(&directory);
        let target = directory.path().join("token-target");
        if let Err(error) = fs::write(&target, VALID_TOKEN) {
            panic!("Device Token target fixture must be written: {error}");
        }
        if let Err(error) = symlink(&target, paths.device_token()) {
            panic!("Device Token symlink fixture must be created: {error}");
        }

        let result = device_token_present(&paths);

        assert!(matches!(result, Err(EnrollmentError::EnrolledArtifacts)));
    }

    #[test]
    fn csr_raw_spki_derivation_is_pinned_to_the_public_csr_fixture() {
        // The fixture is a public CSR (public key + signature, no private material) so
        // the raw-SPKI extraction and hashing stay pinned across implementations
        // without committing a private key.
        let csr_der = fixture_csr_der();
        let Ok(extracted_spki) = raw_csr_spki_der(&csr_der) else {
            panic!("CSR fixture must parse with x509-parser");
        };
        assert_eq!(
            hex::encode(Sha256::digest(extracted_spki)),
            FIXTURE_SPKI_SHA256
        );

        let key = require_key();
        let (csr_der, derived_spki) = match build_csr(&key) {
            Ok(csr) => csr,
            Err(error) => panic!("CSR must be built: {error}"),
        };
        let Ok(extracted_spki) = raw_csr_spki_der(&csr_der) else {
            panic!("generated CSR must parse with x509-parser");
        };

        assert_eq!(derived_spki, extracted_spki);
        assert_eq!(derived_spki, key.subject_public_key_info());
        let (_, parsed) = match X509CertificationRequest::from_der(&csr_der) {
            Ok(parsed) => parsed,
            Err(error) => panic!("CSR fixture must parse: {error}"),
        };
        assert!(
            parsed
                .certification_request_info
                .subject
                .iter()
                .next()
                .is_none()
        );
        assert!(parsed.certification_request_info.attributes().is_empty());
    }

    #[test]
    fn finalization_rejects_spki_mismatch_before_any_write() {
        let directory = require_tempdir();
        let paths = fixture_paths(&directory);
        let (issuer, origin) = require_origin();
        let expected_key = require_key();
        let wrong_key = require_key();
        let leaf = require_leaf(&wrong_key, &issuer, GATEWAY_HOSTNAME);
        let response = issued_response(&leaf, &[origin.as_ref()], VALID_TOKEN);

        let result = finalize_fixture(
            &paths,
            response,
            &expected_key.subject_public_key_info(),
            &origin,
            GATEWAY_HOSTNAME,
        );

        assert!(matches!(result, Err(EnrollmentError::LeafSpkiMismatch)));
        assert_no_finalization_writes(&paths);
    }

    #[test]
    fn finalization_rejects_wrong_chain_before_any_write() {
        let directory = require_tempdir();
        let paths = fixture_paths(&directory);
        let (issuer, origin) = require_origin();
        let (_, wrong_origin) = require_origin();
        let key = require_key();
        let leaf = require_leaf(&key, &issuer, GATEWAY_HOSTNAME);
        let response = issued_response(&leaf, &[wrong_origin.as_ref()], VALID_TOKEN);

        let result = finalize_fixture(
            &paths,
            response,
            &key.subject_public_key_info(),
            &origin,
            GATEWAY_HOSTNAME,
        );

        assert!(matches!(result, Err(EnrollmentError::InvalidChain)));
        assert_no_finalization_writes(&paths);
    }

    #[test]
    fn finalization_rejects_leaf_signed_by_foreign_ca_before_any_write() {
        let directory = require_tempdir();
        let paths = fixture_paths(&directory);
        let (_, origin) = require_origin();
        let (foreign_issuer, _) = require_origin();
        let key = require_key();
        let leaf = require_leaf(&key, &foreign_issuer, GATEWAY_HOSTNAME);
        let response = issued_response(&leaf, &[origin.as_ref()], VALID_TOKEN);

        let result = finalize_fixture(
            &paths,
            response,
            &key.subject_public_key_info(),
            &origin,
            GATEWAY_HOSTNAME,
        );

        assert!(matches!(result, Err(EnrollmentError::InvalidChain)));
        assert_no_finalization_writes(&paths);
    }

    #[test]
    fn finalization_rejects_wrong_hostname_before_any_write() {
        let directory = require_tempdir();
        let paths = fixture_paths(&directory);
        let (issuer, origin) = require_origin();
        let key = require_key();
        let leaf = require_leaf(&key, &issuer, "other.contest.example");
        let response = issued_response(&leaf, &[origin.as_ref()], VALID_TOKEN);

        let result = finalize_fixture(
            &paths,
            response,
            &key.subject_public_key_info(),
            &origin,
            GATEWAY_HOSTNAME,
        );

        assert!(matches!(result, Err(EnrollmentError::InvalidHostname)));
        assert_no_finalization_writes(&paths);
    }

    #[test]
    fn finalization_rejects_expired_leaf_before_any_write() {
        let directory = require_tempdir();
        let paths = fixture_paths(&directory);
        let (issuer, origin) = require_origin();
        let key = require_key();
        let leaf = require_leaf_with_validity(&key, &issuer, GATEWAY_HOSTNAME, 2020, 2021);
        let response = issued_response(&leaf, &[origin.as_ref()], VALID_TOKEN);

        let result = finalize_fixture(
            &paths,
            response,
            &key.subject_public_key_info(),
            &origin,
            GATEWAY_HOSTNAME,
        );

        assert!(matches!(result, Err(EnrollmentError::LeafValidityWindow)));
        assert_no_finalization_writes(&paths);
    }

    #[test]
    fn finalization_rejects_not_yet_valid_leaf_before_any_write() {
        let directory = require_tempdir();
        let paths = fixture_paths(&directory);
        let (issuer, origin) = require_origin();
        let key = require_key();
        let leaf = require_leaf_with_validity(&key, &issuer, GATEWAY_HOSTNAME, 4090, 4091);
        let response = issued_response(&leaf, &[origin.as_ref()], VALID_TOKEN);

        let result = finalize_fixture(
            &paths,
            response,
            &key.subject_public_key_info(),
            &origin,
            GATEWAY_HOSTNAME,
        );

        assert!(matches!(result, Err(EnrollmentError::LeafValidityWindow)));
        assert_no_finalization_writes(&paths);
    }

    #[test]
    fn finalization_rejects_bad_token_shape_before_any_write() {
        let directory = require_tempdir();
        let paths = fixture_paths(&directory);
        let (issuer, origin) = require_origin();
        let key = require_key();
        let leaf = require_leaf(&key, &issuer, GATEWAY_HOSTNAME);
        let response = issued_response(&leaf, &[origin.as_ref()], "bad+token");

        let result = finalize_fixture(
            &paths,
            response,
            &key.subject_public_key_info(),
            &origin,
            GATEWAY_HOSTNAME,
        );

        assert!(matches!(result, Err(EnrollmentError::InvalidToken)));
        assert_no_finalization_writes(&paths);
    }

    #[test]
    fn device_token_shape_pins_the_32_byte_base64url_tail() {
        let invalid_tail = format!("{}B", &VALID_TOKEN[..DEVICE_TOKEN_LENGTH - 1]);

        assert!(valid_device_token(VALID_TOKEN));
        assert!(!valid_device_token(&invalid_tail));
    }

    #[test]
    fn finalization_requires_exactly_one_chain_certificate_before_any_write() {
        let directory = require_tempdir();
        let paths = fixture_paths(&directory);
        let (issuer, origin) = require_origin();
        let key = require_key();
        let leaf = require_leaf(&key, &issuer, GATEWAY_HOSTNAME);
        let response = issued_response(&leaf, &[], VALID_TOKEN);

        let result = finalize_fixture(
            &paths,
            response,
            &key.subject_public_key_info(),
            &origin,
            GATEWAY_HOSTNAME,
        );

        assert!(matches!(result, Err(EnrollmentError::InvalidChain)));
        assert_no_finalization_writes(&paths);
    }

    #[test]
    fn verified_finalization_replaces_all_artifacts_in_frozen_order_modes() {
        let directory = require_tempdir();
        let paths = fixture_paths(&directory);
        for path in [
            paths.gateway_leaf(),
            paths.gateway_chain(),
            paths.device_token(),
        ] {
            if let Err(error) = fs::write(path, b"old artifact with a longer body") {
                panic!("old artifact fixture must be written: {error}");
            }
        }
        let (issuer, origin) = require_origin();
        let key = require_key();
        let leaf = require_leaf(&key, &issuer, GATEWAY_HOSTNAME);
        let response = issued_response(&leaf, &[origin.as_ref()], VALID_TOKEN);

        if let Err(error) = finalize_fixture(
            &paths,
            response,
            &key.subject_public_key_info(),
            &origin,
            GATEWAY_HOSTNAME,
        ) {
            panic!("verified finalization must persist: {error}");
        }

        assert_eq!(require_contents(&paths.gateway_leaf()), leaf);
        assert_eq!(require_contents(&paths.gateway_chain()), origin.as_ref());
        assert_eq!(
            require_contents(&paths.device_token()),
            VALID_TOKEN.as_bytes()
        );
        for (path, mode) in [
            (paths.gateway_leaf(), GATEWAY_ARTIFACT_MODE),
            (paths.gateway_chain(), GATEWAY_ARTIFACT_MODE),
            (paths.device_token(), DEVICE_TOKEN_MODE),
        ] {
            let metadata = match fs::metadata(path) {
                Ok(metadata) => metadata,
                Err(error) => panic!("artifact metadata must be readable: {error}"),
            };
            assert_eq!(metadata.mode() & 0o777, mode);
        }
    }

    #[test]
    fn error_codes_have_frozen_single_step_classifications() {
        let correlation_id = Uuid::now_v7();
        let rejected = serde_json::to_vec(&json!({
            "title": "rejected",
            "status": 409,
            "code": "ENROLLMENT_REQUEST_REJECTED",
            "correlation_id": correlation_id,
        }));
        let closed = serde_json::to_vec(&json!({
            "title": "closed",
            "status": 409,
            "code": "PROVISIONING_WINDOW_CLOSED",
            "correlation_id": correlation_id,
        }));
        let invalid = serde_json::to_vec(&json!({
            "title": "invalid",
            "status": 400,
            "code": "ENROLLMENT_REQUEST_INVALID",
            "correlation_id": correlation_id,
        }));
        let rejected = match rejected {
            Ok(body) => classify_error_response(StatusCode::CONFLICT, &body),
            Err(error) => panic!("rejected fixture must serialize: {error}"),
        };
        let closed = match closed {
            Ok(body) => classify_error_response(StatusCode::CONFLICT, &body),
            Err(error) => panic!("closed fixture must serialize: {error}"),
        };
        let invalid = match invalid {
            Ok(body) => classify_error_response(StatusCode::BAD_REQUEST, &body),
            Err(error) => panic!("invalid fixture must serialize: {error}"),
        };

        assert!(matches!(rejected, Ok(EnrollmentStep::Rejected)));
        assert!(matches!(
            closed,
            Ok(EnrollmentStep::Waiting(
                EnrollmentWaitState::ProvisioningWindowClosed
            ))
        ));
        assert!(matches!(invalid, Err(EnrollmentError::RequestInvalid)));
    }
}
