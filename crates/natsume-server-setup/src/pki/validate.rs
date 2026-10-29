//! PKI material validation through the Server's authoritative setup policy.

use std::{net::IpAddr, time::SystemTime};

use natsume_server::{DeploymentError, DeploymentMaterials, config::ServerConfig};

use super::{Certificate, PrivateKey};

/// Validates normalized installation materials without writing deployment files.
///
/// Uses the Origin CA's exact DER as the packaged local Origin trust root.
/// All identity, certificate policy, IP, and validity checks belong to the Server facade.
///
/// # Errors
/// Returns the Server facade's redacted, classified deployment rejection.
#[expect(
    clippy::too_many_arguments,
    reason = "setup facade takes the six independently selected materials"
)]
pub fn validate_materials(
    config: &ServerConfig,
    control: &Certificate,
    leaf: &Certificate,
    leaf_key: &PrivateKey,
    origin: &Certificate,
    origin_key: &PrivateKey,
    ips: &[IpAddr],
    now: SystemTime,
) -> Result<(), DeploymentError> {
    natsume_server::validate_setup_materials(
        config,
        DeploymentMaterials {
            control_root_der: control.der(),
            tls_leaf_der: leaf.der(),
            tls_key_pkcs8_der: leaf_key.pkcs8_der(),
            origin_ca_der: origin.der(),
            origin_key_pkcs8_der: origin_key.pkcs8_der(),
            local_origin_root_der: origin.der(),
        },
        ips,
        now,
    )
}
