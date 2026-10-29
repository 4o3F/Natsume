//! PKI primitives for first-install setup.

pub mod generate;
pub mod import;
pub mod pending;
pub mod rules;
pub mod validate;

use std::{fmt, net::IpAddr};

use rcgen::{KeyPair, PublicKeyData as _};
use rustls_pki_types::PrivatePkcs8KeyDer;
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;
use zeroize::Zeroizing;

/// A single, completely parsed X.509 certificate in DER encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    der: Vec<u8>,
    pem: String,
    spki: Vec<u8>,
    subject: String,
    not_before: OffsetDateTime,
    not_after: OffsetDateTime,
    ip_sans: Vec<IpAddr>,
    is_ca: bool,
    self_signed: bool,
}

impl Certificate {
    /// Borrows the complete DER certificate.
    #[must_use]
    pub fn der(&self) -> &[u8] {
        &self.der
    }

    /// Encodes the certificate as a single CERTIFICATE PEM block.
    #[must_use]
    pub fn to_pem(&self) -> String {
        self.pem.clone()
    }

    /// Returns the SHA-256 certificate fingerprint as colon-separated uppercase hex.
    #[must_use]
    pub fn sha256_fingerprint(&self) -> String {
        Sha256::digest(&self.der)
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// Hashes the complete DER `SubjectPublicKeyInfo` with SHA-256.
    #[must_use]
    pub fn spki_sha256(&self) -> [u8; 32] {
        Sha256::digest(&self.spki).into()
    }

    /// Formats the certificate's subject distinguished name for display.
    #[must_use]
    pub fn subject(&self) -> String {
        self.subject.clone()
    }

    /// Returns the beginning of the certificate's validity interval.
    #[must_use]
    pub fn not_before(&self) -> OffsetDateTime {
        self.not_before
    }

    /// Returns the end of the certificate's validity interval.
    #[must_use]
    pub fn not_after(&self) -> OffsetDateTime {
        self.not_after
    }

    /// Returns IP subject alternative names in their encoded order, without DNS names.
    #[must_use]
    pub fn ip_sans(&self) -> Vec<IpAddr> {
        self.ip_sans.clone()
    }

    /// Reports whether basicConstraints marks the certificate as a CA.
    #[must_use]
    pub fn is_ca(&self) -> bool {
        self.is_ca
    }

    /// Checks both issuer/subject equality and the signature against its own public key.
    #[must_use]
    pub fn is_self_signed(&self) -> bool {
        self.self_signed
    }

    #[cfg(test)]
    fn parsed(&self) -> Result<x509_parser::certificate::X509Certificate<'_>, import::ImportError> {
        use snafu::ResultExt as _;
        x509_parser::parse_x509_certificate(&self.der)
            .map(|(_, certificate)| certificate)
            .context(import::InvalidCertificateSnafu)
    }
}

/// The elliptic curve family recognized in an imported PKCS#8 algorithm identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAlgorithm {
    /// ECDSA over the named NIST P-256 curve.
    P256,
    /// ECDSA over the named NIST P-384 curve.
    P384,
    /// Another private key algorithm.
    Other,
}

/// An unencrypted PKCS#8 private key whose owned bytes are erased on drop.
pub struct PrivateKey {
    der: Zeroizing<Vec<u8>>,
    algorithm: KeyAlgorithm,
    spki: Vec<u8>,
}

impl fmt::Debug for PrivateKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PrivateKey { <redacted> }")
    }
}

impl PrivateKey {
    /// Borrows secret PKCS#8 DER; callers must not log or display these bytes.
    #[must_use]
    pub fn pkcs8_der(&self) -> &[u8] {
        &self.der
    }

    /// Identifies P-256 and P-384 using the named curve in the algorithm identifier.
    #[must_use]
    pub fn algorithm(&self) -> KeyAlgorithm {
        self.algorithm
    }

    /// Compares the derived public key with the certificate's `SubjectPublicKeyInfo`.
    #[must_use]
    pub fn matches(&self, certificate: &Certificate) -> bool {
        self.spki == certificate.spki
    }

    fn from_key_pair(key: &KeyPair) -> Self {
        let algorithm = if key.algorithm() == &rcgen::PKCS_ECDSA_P256_SHA256 {
            KeyAlgorithm::P256
        } else if key.algorithm() == &rcgen::PKCS_ECDSA_P384_SHA384 {
            KeyAlgorithm::P384
        } else {
            KeyAlgorithm::Other
        };
        Self {
            der: Zeroizing::new(key.serialize_der()),
            algorithm,
            spki: key.subject_public_key_info(),
        }
    }

    fn key_pair(&self) -> Result<KeyPair, import::ImportError> {
        use snafu::ResultExt as _;
        KeyPair::try_from(&PrivatePkcs8KeyDer::from(self.pkcs8_der()))
            .context(import::KeyAlgorithmSnafu)
    }
}

#[cfg(test)]
mod tests;
