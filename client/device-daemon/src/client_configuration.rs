#![allow(dead_code)]

use std::{fs, path::Path};

use rustls_pki_types::{CertificateDer, pem::PemObject as _};
use serde::Deserialize;
use snafu::Snafu;
use x509_parser::{certificate::X509Certificate, prelude::FromDer as _};

use crate::{CanonicalEndpoint, parse_endpoint};

pub(crate) const CLIENT_CONFIG_PATH: &str = "/etc/natsume/config.toml";
pub(crate) const CONTROL_ROOT_PATH: &str = "/etc/natsume/trust/control-ca.crt";
pub(crate) const KEYS_DIRECTORY_PATH: &str = "/var/lib/natsume/keys";
pub(crate) const DEVICE_TOKEN_NAME: &str = "device-token";

#[derive(Debug, Snafu)]
pub(crate) enum ClientConfigurationError {
    #[snafu(display("the client endpoint configuration is invalid"))]
    Endpoint,

    #[snafu(display("the client trust root is invalid"))]
    TrustRoot,
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

pub(crate) fn read_endpoint(path: &Path) -> Result<CanonicalEndpoint, ClientConfigurationError> {
    let encoded = fs::read_to_string(path).map_err(|_| ClientConfigurationError::Endpoint)?;
    let config =
        toml::from_str::<ClientConfig>(&encoded).map_err(|_| ClientConfigurationError::Endpoint)?;
    parse_endpoint(&config.server.ip, &config.server.port.to_string())
        .map_err(|_| ClientConfigurationError::Endpoint)
}

pub(crate) fn read_single_pem_certificate(
    path: &Path,
) -> Result<CertificateDer<'static>, ClientConfigurationError> {
    let encoded = fs::read(path).map_err(|_| ClientConfigurationError::TrustRoot)?;
    let mut certificates = CertificateDer::pem_slice_iter(&encoded);
    let certificate = certificates
        .next()
        .ok_or(ClientConfigurationError::TrustRoot)?
        .map_err(|_| ClientConfigurationError::TrustRoot)?;
    let parsed = X509Certificate::from_der(certificate.as_ref());
    if certificates.next().is_some()
        || !matches!(parsed, Ok((remainder, _certificate)) if remainder.is_empty())
    {
        return Err(ClientConfigurationError::TrustRoot);
    }
    Ok(certificate)
}
