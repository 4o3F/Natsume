//! Certificate, CSR, and encrypted private key generation for first-install setup.

use std::net::IpAddr;

use pkcs8::{PrivateKeyInfoRef, der::Decode as _};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, SanType,
};
use rustls_pki_types::CertificateDer;
use secrecy::{ExposeSecret as _, SecretString};
use snafu::{ResultExt as _, Snafu, ensure};
use time::{Duration, OffsetDateTime};
use zeroize::Zeroizing;

use super::{
    Certificate, PrivateKey,
    import::{self, ImportError},
};

/// PBKDF2-HMAC-SHA256 iterations for exported CA keys; increasing this raises unlock CPU cost.
pub const ENCRYPTION_ITERATIONS: u32 = 600_000;

/// A certificate generation or private key encryption failure, without secret material.
#[derive(Debug, Snafu)]
pub enum GenerateError {
    /// The certificate or CSR provider failed.
    #[snafu(display("certificate or signing request generation failed"))]
    Certificate {
        /// The cryptographic provider's cause.
        source: rcgen::Error,
    },
    /// Generated material could not be normalized.
    #[snafu(display("generated certificate or key normalization failed"))]
    Import {
        /// The material parser's cause.
        source: ImportError,
    },
    /// The requested leaf cannot cover the Gateway expiry.
    #[snafu(display("server certificate validity cannot cover the Gateway expiry"))]
    InsufficientValidity,
    /// The supplied key does not belong to the issuing CA.
    #[snafu(display("issuing CA certificate and private key do not match"))]
    IssuerKeyMismatch,
    /// Operating system randomness could not be read.
    #[snafu(display("operating system randomness unavailable"))]
    Randomness {
        /// The random provider's cause.
        source: getrandom::Error,
    },
    /// PKCS#8 could not be decoded or encoded.
    #[snafu(display("private key DER encoding failed"))]
    Der {
        /// The DER codec's cause.
        source: pkcs8::der::Error,
    },
    /// Explicit PBES2 parameters could not be constructed.
    #[snafu(display("private key encryption parameters rejected"))]
    Parameters {
        /// The PBES2 provider's cause.
        source: pkcs5::Error,
    },
    /// PKCS#8 encryption failed.
    #[snafu(display("private key encryption failed"))]
    Encryption {
        /// The PKCS#8 provider's cause.
        source: pkcs8::Error,
    },
}

/// A PKCS#10 request containing IP SANs and a serverAuth extension request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningRequest {
    pem: String,
}

impl SigningRequest {
    /// Borrows the PEM signing request for offline signing.
    #[must_use]
    pub fn pem(&self) -> &str {
        &self.pem
    }
}

/// Generates a P-256 self-signed CA with pathlen zero and a ten-year validity interval.
///
/// The CA permits certificate and CRL signing, includes SKI, and starts one hour before `now`.
///
/// # Errors
/// Returns a provider or generated-material parsing error.
pub fn generate_ca(
    common_name: &str,
    organization: &str,
    now: OffsetDateTime,
) -> Result<(Certificate, PrivateKey), GenerateError> {
    let key =
        Zeroizing::new(KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).context(CertificateSnafu)?);
    let mut params = CertificateParams::default();
    params.not_before = now - Duration::hours(1);
    params.not_after = now + Duration::days(3650);
    params.distinguished_name = distinguished_name(common_name, organization);
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let certificate = params.self_signed(&*key).context(CertificateSnafu)?;
    normalize(&certificate, &key)
}

/// Issues a new P-256 server leaf with every supplied IP SAN and no DNS SANs.
///
/// Expiry is the earlier of one year after `now` and the issuing CA's expiry.
/// Includes critical CA:FALSE and digitalSignature, serverAuth, SKI, and AKI.
///
/// # Errors
/// Returns [`GenerateError::InsufficientValidity`] if expiry cannot cover the Gateway,
/// a mismatched issuer key error, or a certificate provider/parsing error.
pub fn issue_server_leaf(
    ca_cert: &Certificate,
    ca_key: &PrivateKey,
    ips: &[IpAddr],
    gateway_not_after: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<(Certificate, PrivateKey), GenerateError> {
    ensure!(ca_key.matches(ca_cert), IssuerKeyMismatchSnafu);
    let not_after = (now + Duration::days(365)).min(ca_cert.not_after());
    ensure!(not_after >= gateway_not_after, InsufficientValiditySnafu);
    let issuer_key = ca_key.key_pair().context(ImportSnafu)?;
    let issuer = Issuer::from_ca_cert_der(&CertificateDer::from(ca_cert.der()), &*issuer_key)
        .context(CertificateSnafu)?;
    let key =
        Zeroizing::new(KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).context(CertificateSnafu)?);
    let mut params = server_params(ips);
    params.not_before = now - Duration::hours(1);
    params.not_after = not_after;
    params.use_authority_key_identifier_extension = true;
    let certificate = params.signed_by(&*key, &issuer).context(CertificateSnafu)?;
    let (certificate, key) = normalize(&certificate, &key)?;
    ensure!(
        certificate.not_after() >= gateway_not_after,
        InsufficientValiditySnafu
    );
    Ok((certificate, key))
}

/// Creates a new P-256 private key and signed PKCS#10 request for offline issuance.
///
/// # Errors
/// Returns a cryptographic provider or private key normalization error.
pub fn create_csr(ips: &[IpAddr]) -> Result<(PrivateKey, SigningRequest), GenerateError> {
    let key =
        Zeroizing::new(KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).context(CertificateSnafu)?);
    let request = server_params(ips)
        .serialize_request(&*key)
        .context(CertificateSnafu)?;
    let request = SigningRequest {
        pem: request.pem().context(CertificateSnafu)?,
    };
    let private = PrivateKey::from_key_pair(&key);
    Ok((private, request))
}

/// Exports PKCS#8 using PBKDF2-HMAC-SHA256 and AES-256-CBC with explicit parameters.
///
/// Uses [`ENCRYPTION_ITERATIONS`], an independent random 16-byte salt and 16-byte IV
/// from the operating system random source, and the ENCRYPTED PRIVATE KEY PEM label.
///
/// # Errors
/// Returns a randomness, encoding, parameter, or encryption failure.
pub fn encrypt_private_key_pem(
    key: &PrivateKey,
    passphrase: &SecretString,
) -> Result<Zeroizing<String>, GenerateError> {
    let mut random = [0; 32];
    getrandom::fill(&mut random).context(RandomnessSnafu)?;
    let mut iv = [0; 16];
    iv.copy_from_slice(&random[16..]);
    let parameters = pkcs5::pbes2::Parameters::generate_pbkdf2_sha256_aes256cbc(
        ENCRYPTION_ITERATIONS,
        &random[..16],
        iv,
    )
    .context(ParametersSnafu)?;
    let info = PrivateKeyInfoRef::from_der(key.pkcs8_der()).context(DerSnafu)?;
    info.encrypt_with_params(parameters, passphrase.expose_secret().as_bytes())
        .context(EncryptionSnafu)?
        .to_pem("ENCRYPTED PRIVATE KEY", pkcs8::LineEnding::LF)
        .context(DerSnafu)
}

fn distinguished_name(common_name: &str, organization: &str) -> DistinguishedName {
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, common_name);
    name.push(DnType::OrganizationName, organization);
    name
}

fn server_params(ips: &[IpAddr]) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name = distinguished_name("Natsume Server", "Natsume");
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.subject_alt_names = ips.iter().copied().map(SanType::IpAddress).collect();
    params
}

fn normalize(
    certificate: &rcgen::Certificate,
    key: &KeyPair,
) -> Result<(Certificate, PrivateKey), GenerateError> {
    let certificate = import::import_certificate(certificate.der()).context(ImportSnafu)?;
    Ok((certificate, PrivateKey::from_key_pair(key)))
}
