//! Certificate and private key import for first-install setup.

use pkcs5::{EncryptionScheme, pbes2::Kdf};
use pkcs8::{
    EncryptedPrivateKeyInfoRef, PrivateKeyInfoRef,
    der::{Decode as _, Encode as _, asn1::OctetStringRef},
};
use rcgen::KeyPair;
use rustls_pki_types::PrivatePkcs8KeyDer;
use secrecy::{ExposeSecret as _, SecretString};
use snafu::{OptionExt as _, ResultExt as _, Snafu, ensure};
use zeroize::Zeroizing;

use super::{Certificate, PrivateKey};

/// Maximum accepted PBKDF2 iterations; bounds CPU work before any decryption.
pub const MAX_PBKDF2_ITERATIONS: u32 = 2_000_000;
/// Maximum scrypt N; with r <= 8 this bounds working memory to about 128 MiB.
pub const MAX_SCRYPT_N: u64 = 131_072;
/// Maximum scrypt block size, limiting memory per unit of N.
pub const MAX_SCRYPT_R: u16 = 8;
/// Maximum scrypt parallelization, limiting total CPU work with the N and r caps.
pub const MAX_SCRYPT_P: u16 = 4;

/// A redacted import failure retaining parser or cryptographic causes where applicable.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub enum ImportError {
    /// The input could not be parsed as an X.509 certificate.
    #[snafu(display("invalid certificate"))]
    InvalidCertificate {
        /// The certificate parser's non-secret cause.
        source: x509_parser::asn1_rs::Err<x509_parser::error::X509Error>,
    },
    /// A certificate extension is malformed or duplicated.
    #[snafu(display("invalid certificate extension"))]
    InvalidExtension {
        /// The extension parser's cause.
        source: x509_parser::error::X509Error,
    },
    /// An extension contains malformed ASN.1.
    #[snafu(display("malformed certificate extension"))]
    MalformedExtension {
        /// The ASN.1 parser cause.
        source: x509_parser::asn1_rs::Err<x509_parser::asn1_rs::Error>,
    },
    /// More than one certificate PEM block was supplied.
    #[snafu(display("expected exactly one certificate"))]
    MultipleCertificates,
    /// Bytes remain after the certificate or PEM block.
    #[snafu(display("trailing data after certificate or key"))]
    TrailingData,
    /// A PEM envelope could not be decoded.
    #[snafu(display("invalid PEM encoding"))]
    InvalidPem {
        /// The PEM decoder's cause.
        source: pkcs8::der::pem::Error,
    },
    /// A certificate PEM block has an unexpected label.
    #[snafu(display("expected a CERTIFICATE PEM block"))]
    CertificateLabel,
    /// An encrypted PKCS#8 key requires a passphrase.
    #[snafu(display("a passphrase is required for this private key"))]
    PassphraseRequired,
    /// Decryption failed with the supplied passphrase.
    #[snafu(display("private key decryption failed; check the passphrase"))]
    WrongPassphrase {
        /// The decryption cause, without any passphrase or key bytes.
        source: pkcs8::Error,
    },
    /// Traditional OpenSSL PEM encryption must first be converted to PKCS#8.
    #[snafu(display(
        "legacy encrypted PEM is unsupported; convert with: openssl pkcs8 -topk8 -v2 aes-256-cbc -v2prf hmacWithSHA256 -in <old> -out <new>"
    ))]
    LegacyEncryptedPem,
    /// The input is not a supported private key encoding.
    #[snafu(display("unsupported private key; use PKCS#8 or named-curve SEC1"))]
    UnsupportedKey {
        /// The format parser's cause.
        #[snafu(source(from(pkcs8::der::Error, KeyEncodingError::from)))]
        source: KeyEncodingError,
    },
    /// The PKCS#8 key cannot be loaded by the cryptographic provider.
    #[snafu(display("unsupported or invalid private key algorithm"))]
    KeyAlgorithm {
        /// The provider's cause.
        source: rcgen::Error,
    },
    /// SEC1 lacks the named curve required for conversion.
    #[snafu(display("SEC1 private key must include a named curve"))]
    MissingNamedCurve,
    /// Encryption is not PBES2.
    #[snafu(display("private key encryption must use PBES2"))]
    UnsupportedEncryption,
    /// The KDF requests excessive resources or invalid cost parameters.
    #[snafu(display("encrypted private key exceeds KDF resource limits"))]
    KdfLimitExceeded,
}

/// The underlying reason a private key encoding is unsupported.
#[derive(Debug, Snafu)]
pub enum KeyEncodingError {
    /// The ASN.1 encoding does not match the required key format.
    #[snafu(context(false), display("invalid private key DER"))]
    Der {
        /// The DER parser cause.
        source: pkcs8::der::Error,
    },
    /// The PEM label denotes an unsupported format such as RSA PKCS#1.
    #[snafu(display("unsupported private key PEM label"))]
    UnsupportedLabel,
}

/// Imports exactly one PEM or DER certificate, rejecting trailing DER or extra blocks.
///
/// # Errors
/// Returns a classified parsing, extension, multiplicity, or trailing-data error.
pub fn import_certificate(bytes: &[u8]) -> Result<Certificate, ImportError> {
    let der = if bytes.trim_ascii_start().starts_with(b"-----BEGIN ") {
        let marker = b"-----BEGIN CERTIFICATE-----";
        ensure!(
            bytes
                .windows(marker.len())
                .filter(|value| *value == marker)
                .count()
                <= 1,
            MultipleCertificatesSnafu
        );
        let (label, der) = decode_pem(bytes)?;
        ensure!(label == "CERTIFICATE", CertificateLabelSnafu);
        der.to_vec()
    } else {
        bytes.to_vec()
    };
    let (remaining, certificate) =
        x509_parser::parse_x509_certificate(&der).context(InvalidCertificateSnafu)?;
    ensure!(remaining.is_empty(), TrailingDataSnafu);
    certificate
        .extensions_map()
        .context(InvalidExtensionSnafu)?;
    certificate
        .basic_constraints()
        .context(InvalidExtensionSnafu)?;
    certificate
        .subject_alternative_name()
        .context(InvalidExtensionSnafu)?;
    for extension in certificate.extensions() {
        if let x509_parser::extensions::ParsedExtension::ParseError { error } =
            extension.parsed_extension()
        {
            return Err(ImportError::MalformedExtension {
                source: error.clone(),
            });
        }
    }
    let subject = certificate.subject().to_string();
    let not_before = certificate.validity().not_before.to_datetime();
    let not_after = certificate.validity().not_after.to_datetime();
    let spki = certificate.public_key().raw.to_vec();
    let is_ca = certificate.is_ca();
    let self_signed =
        certificate.subject() == certificate.issuer() && certificate.verify_signature(None).is_ok();
    let ip_sans = certificate
        .subject_alternative_name()
        .context(InvalidExtensionSnafu)?
        .map(|sans| {
            sans.value
                .general_names
                .iter()
                .filter_map(|name| {
                    let x509_parser::extensions::GeneralName::IPAddress(bytes) = name else {
                        return None;
                    };
                    match *bytes {
                        [a, b, c, d] => Some(std::net::IpAddr::from([*a, *b, *c, *d])),
                        bytes if bytes.len() == 16 => {
                            let mut octets = [0; 16];
                            octets.copy_from_slice(bytes);
                            Some(std::net::IpAddr::from(octets))
                        }
                        _ => None,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let pem = pkcs8::der::pem::encode_string("CERTIFICATE", pkcs8::LineEnding::LF, &der)
        .context(InvalidPemSnafu)?;
    Ok(Certificate {
        der,
        pem,
        spki,
        subject,
        not_before,
        not_after,
        ip_sans,
        is_ca,
        self_signed,
    })
}

/// Imports plaintext PKCS#8, PBES2 encrypted PKCS#8, or named-curve plaintext SEC1.
///
/// KDF limits are checked before decryption, even when no passphrase was provided.
/// Temporary decoded and normalized private key buffers are erased on drop.
/// SEC1 keys must include their public key for the ring provider; OpenSSL includes it by default.
///
/// # Errors
/// Returns a classified format, encryption, passphrase, or resource-limit error.
/// SEC1 keys without a public key return [`ImportError::KeyAlgorithm`].
pub fn import_private_key(
    bytes: &[u8],
    passphrase: Option<&SecretString>,
) -> Result<PrivateKey, ImportError> {
    if bytes.trim_ascii_start().starts_with(b"-----BEGIN ") {
        ensure!(
            !bytes
                .split(|byte| *byte == b'\n')
                .any(|line| line.trim_ascii() == b"Proc-Type: 4,ENCRYPTED"),
            LegacyEncryptedPemSnafu
        );
        let (label, der) = decode_pem(bytes)?;
        return match label {
            "PRIVATE KEY" => from_pkcs8(&der),
            "ENCRYPTED PRIVATE KEY" => decrypt(&der, passphrase),
            "EC PRIVATE KEY" => from_sec1(&der),
            _ => Err(ImportError::UnsupportedKey {
                source: KeyEncodingError::UnsupportedLabel,
            }),
        };
    }
    if PrivateKeyInfoRef::from_der(bytes).is_ok() {
        from_pkcs8(bytes)
    } else if EncryptedPrivateKeyInfoRef::from_der(bytes).is_ok() {
        decrypt(bytes, passphrase)
    } else {
        from_sec1(bytes)
    }
}

fn decode_pem(bytes: &[u8]) -> Result<(&str, Zeroizing<Vec<u8>>), ImportError> {
    let bytes = bytes.trim_ascii();
    let marker = b"-----END ";
    if let Some(start) = bytes
        .windows(marker.len())
        .position(|value| value == marker)
        && let Some(end) = bytes[start + marker.len()..]
            .windows(5)
            .position(|value| value == b"-----")
    {
        let end = start + marker.len() + end + 5;
        ensure!(bytes[end..].trim_ascii().is_empty(), TrailingDataSnafu);
    }
    let (label, der) = pkcs8::der::pem::decode_vec(bytes).context(InvalidPemSnafu)?;
    Ok((label, Zeroizing::new(der)))
}

fn from_pkcs8(der: &[u8]) -> Result<PrivateKey, ImportError> {
    PrivateKeyInfoRef::from_der(der).context(UnsupportedKeySnafu)?;
    let key = KeyPair::try_from(&PrivatePkcs8KeyDer::from(der)).context(KeyAlgorithmSnafu)?;
    Ok(PrivateKey::from_key_pair(&key))
}

/// Converts named-curve SEC1 keys containing the public key required by ring.
///
/// OpenSSL includes the public key by default.
///
/// # Errors
/// Returns [`ImportError::KeyAlgorithm`] if the public key is absent, or a parsing or curve error.
fn from_sec1(bytes: &[u8]) -> Result<PrivateKey, ImportError> {
    let key = sec1::EcPrivateKey::from_der(bytes).context(UnsupportedKeySnafu)?;
    let parameters = key.parameters.context(MissingNamedCurveSnafu)?;
    let info = PrivateKeyInfoRef::new(
        pkcs8::AlgorithmIdentifierRef {
            oid: sec1::ALGORITHM_OID,
            parameters: Some((&parameters).into()),
        },
        OctetStringRef::new(bytes).context(UnsupportedKeySnafu)?,
    );
    from_pkcs8(&Zeroizing::new(info.to_der().context(UnsupportedKeySnafu)?))
}

fn decrypt(bytes: &[u8], passphrase: Option<&SecretString>) -> Result<PrivateKey, ImportError> {
    let encrypted = EncryptedPrivateKeyInfoRef::from_der(bytes).context(UnsupportedKeySnafu)?;
    let EncryptionScheme::Pbes2(parameters) = &encrypted.encryption_algorithm else {
        return UnsupportedEncryptionSnafu.fail();
    };
    let permitted = match &parameters.kdf {
        Kdf::Pbkdf2(params) => (1..=MAX_PBKDF2_ITERATIONS).contains(&params.iteration_count),
        Kdf::Scrypt(params) => {
            params.cost_parameter.is_power_of_two()
                && (2..=MAX_SCRYPT_N).contains(&params.cost_parameter)
                && (1..=MAX_SCRYPT_R).contains(&params.block_size)
                && (1..=MAX_SCRYPT_P).contains(&params.parallelization)
        }
        _ => false,
    };
    ensure!(permitted, KdfLimitExceededSnafu);
    let passphrase = passphrase.context(PassphraseRequiredSnafu)?;
    let document = encrypted
        .decrypt(passphrase.expose_secret().as_bytes())
        .context(WrongPassphraseSnafu)?;
    from_pkcs8(document.as_bytes())
}
