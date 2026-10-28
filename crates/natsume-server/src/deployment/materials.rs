use std::{fmt, net::IpAddr, sync::Arc, time::SystemTime};

use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::{RootCertStore, client::WebPkiServerVerifier};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use snafu::ResultExt as _;
use time::OffsetDateTime;
use x509_parser::{
    certificate::X509Certificate,
    oid_registry::{OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY},
    parse_x509_certificate,
};

use crate::{component::gateway::GatewayComponent, config::ServerConfig, tls};

use super::error::{self, CertificateRejection, DeploymentFailure, MaterialRole};

/// Borrowed, normalized deployment inputs; validation never persists these bytes.
#[derive(Clone, Copy)]
pub struct DeploymentMaterials<'a> {
    /// One DER Control CA certificate, used as the sole direct TLS trust anchor.
    pub control_root_der: &'a [u8],
    /// One DER Server TLS leaf certificate.
    pub tls_leaf_der: &'a [u8],
    /// The Server TLS private key in unencrypted PKCS#8 DER.
    pub tls_key_pkcs8_der: &'a [u8],
    /// One DER online Origin CA certificate.
    pub origin_ca_der: &'a [u8],
    /// The online Origin CA private key in unencrypted PKCS#8 DER.
    pub origin_key_pkcs8_der: &'a [u8],
    /// One DER packaged Origin CA trust anchor, identical to the online certificate.
    pub local_origin_root_der: &'a [u8],
}

impl fmt::Debug for DeploymentMaterials<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeploymentMaterials { <redacted> }")
    }
}

pub(super) fn validate(
    config: &ServerConfig,
    materials: DeploymentMaterials<'_>,
    server_ips: &[IpAddr],
    now: SystemTime,
) -> Result<(), DeploymentFailure> {
    use rustls::client::danger::ServerCertVerifier as _;

    tls::server_config_from_der(
        CertificateDer::from(materials.tls_leaf_der.to_vec()),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            materials.tls_key_pkcs8_der.to_vec(),
        )),
    )
    .context(error::TlsSnafu)?;
    GatewayComponent::validate_der(
        &CertificateDer::from(materials.origin_ca_der),
        &PrivatePkcs8KeyDer::from(materials.origin_key_pkcs8_der),
        &CertificateDer::from(materials.local_origin_root_der),
        config.site(),
    )
    .context(error::OriginCaSnafu)?;

    let control = certificate(materials.control_root_der, MaterialRole::ControlCa)?;
    let leaf = certificate(materials.tls_leaf_der, MaterialRole::TlsLeaf)?;
    let origin = certificate(materials.origin_ca_der, MaterialRole::OriginCa)?;
    certificate(
        materials.local_origin_root_der,
        MaterialRole::LocalOriginRoot,
    )?;
    validate_ca(&control, MaterialRole::ControlCa)?;
    validate_ca(&origin, MaterialRole::OriginCa)?;

    validate_origin_key(&origin, materials.origin_key_pkcs8_der)?;
    if materials.origin_ca_der != materials.local_origin_root_der {
        return Err(rejection(
            MaterialRole::OriginCa,
            CertificateRejection::TrustRootMismatch,
        ));
    }

    snafu::ensure!(!server_ips.is_empty(), error::EmptyServerIpsSnafu);
    let elapsed = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .context(error::SystemTimeSnafu)?;
    let timestamp = OffsetDateTime::from_unix_timestamp_nanos(
        i128::try_from(elapsed.as_nanos()).context(error::TimeIntegerSnafu)?,
    )
    .context(error::TimeRangeSnafu)?;
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(materials.control_root_der))
        .context(error::VerifyLeafSnafu)?;
    let verifier = WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .build()
    .context(error::VerifierSnafu)?;
    for ip in server_ips {
        verifier
            .verify_server_cert(
                &CertificateDer::from(materials.tls_leaf_der),
                &[],
                &ServerName::IpAddress((*ip).into()),
                &[],
                UnixTime::since_unix_epoch(elapsed),
            )
            .context(error::VerifyLeafSnafu)?;
    }

    let gateway_not_after = config.site().gateway_not_after().timestamp();
    if leaf.validity().not_after.to_datetime() < gateway_not_after {
        return Err(rejection(
            MaterialRole::TlsLeaf,
            CertificateRejection::InsufficientValidity,
        ));
    }
    if leaf.validity().not_after > control.validity().not_after {
        return Err(rejection(
            MaterialRole::TlsLeaf,
            CertificateRejection::ExceedsControlCaValidity,
        ));
    }
    for (ca, role) in [
        (&control, MaterialRole::ControlCa),
        (&origin, MaterialRole::OriginCa),
    ] {
        if ca.validity().not_before.to_datetime() > timestamp
            || ca.validity().not_after.to_datetime() < timestamp
        {
            return Err(rejection(role, CertificateRejection::NotCurrentlyValid));
        }
    }
    if origin.validity().not_after.to_datetime() < gateway_not_after {
        return Err(rejection(
            MaterialRole::OriginCa,
            CertificateRejection::InsufficientValidity,
        ));
    }
    snafu::ensure!(gateway_not_after > timestamp, error::GatewayExpiredSnafu);
    Ok(())
}

fn validate_origin_key(
    origin: &X509Certificate<'_>,
    key_der: &[u8],
) -> Result<(), DeploymentFailure> {
    let public_key = origin.public_key();
    let key =
        KeyPair::try_from(&PrivatePkcs8KeyDer::from(key_der)).context(error::OriginKeySnafu)?;
    if public_key.algorithm.algorithm != OID_KEY_TYPE_EC_PUBLIC_KEY
        || public_key
            .algorithm
            .parameters
            .as_ref()
            .and_then(|parameters| parameters.as_oid().ok())
            .as_ref()
            != Some(&OID_EC_P256)
        || key.algorithm() != &PKCS_ECDSA_P256_SHA256
    {
        return Err(rejection(
            MaterialRole::OriginCa,
            CertificateRejection::NotP256,
        ));
    }
    Ok(())
}

fn certificate(der: &[u8], role: MaterialRole) -> Result<X509Certificate<'_>, DeploymentFailure> {
    let (remainder, certificate) =
        parse_x509_certificate(der).context(error::ParseCertificateSnafu { role })?;
    if !remainder.is_empty() {
        return Err(rejection(role, CertificateRejection::InvalidDer));
    }
    Ok(certificate)
}

fn validate_ca(ca: &X509Certificate<'_>, role: MaterialRole) -> Result<(), DeploymentFailure> {
    if !ca
        .basic_constraints()
        .context(error::ParseExtensionSnafu { role })?
        .is_some_and(|constraints| constraints.value.ca)
    {
        return Err(rejection(role, CertificateRejection::NotCa));
    }
    if !ca
        .key_usage()
        .context(error::ParseExtensionSnafu { role })?
        .is_some_and(|usage| usage.value.key_cert_sign())
    {
        return Err(rejection(role, CertificateRejection::MissingKeyCertSign));
    }
    Ok(())
}

fn rejection(role: MaterialRole, rejection: CertificateRejection) -> DeploymentFailure {
    error::CertificateSnafu { role, rejection }.build()
}
