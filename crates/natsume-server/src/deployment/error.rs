use std::fmt;

use snafu::Snafu;

/// The deployment material associated with a validation rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialRole {
    /// The direct trust anchor for Server TLS.
    ControlCa,
    /// The Server TLS certificate.
    TlsLeaf,
    /// The Server TLS private key.
    TlsKey,
    /// The online Gateway issuing CA and its key.
    OriginCa,
    /// The packaged Gateway trust anchor.
    LocalOriginRoot,
}

/// A finite, non-secret reason suitable for setup's user-facing explanations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CertificateRejection {
    /// Input is not exactly one complete DER certificate.
    InvalidDer,
    /// Runtime identity loading or key matching failed.
    InvalidIdentity,
    /// The certificate lacks CA basic constraints.
    NotCa,
    /// The CA lacks an explicit keyCertSign key usage.
    MissingKeyCertSign,
    /// The Origin CA certificate or key is not ECDSA P-256.
    NotP256,
    /// The online Origin CA differs from its packaged trust anchor.
    TrustRootMismatch,
    /// The TLS leaf does not cover a configured Server IP.
    IpNotCovered,
    /// The TLS leaf issuer or signature is not trusted by the Control CA.
    UntrustedIssuer,
    /// The TLS leaf is expired or not yet valid.
    LeafNotCurrentlyValid,
    /// The TLS leaf does not permit server authentication.
    NotServerAuth,
    /// Another Server TLS certificate verification check failed.
    ServerVerification,
    /// The certificate does not cover the configured Gateway expiry.
    InsufficientValidity,
    /// The TLS leaf outlives its Control CA.
    ExceedsControlCaValidity,
    /// The CA is not valid at the supplied time.
    NotCurrentlyValid,
}

/// A redacted deployment failure with optional material and policy classification.
#[derive(Snafu)]
#[snafu(context(false), display("{source}"))]
pub struct DeploymentError {
    source: DeploymentFailure,
}

impl fmt::Debug for DeploymentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeploymentError")
            .field("message", &format_args!("{self}"))
            .field("material_role", &self.material_role())
            .field("certificate_rejection", &self.certificate_rejection())
            .finish()
    }
}

impl DeploymentError {
    /// Returns the material role when this failure concerns certificate material.
    #[must_use]
    pub fn material_role(&self) -> Option<MaterialRole> {
        match &self.source {
            DeploymentFailure::Certificate { role, .. }
            | DeploymentFailure::ParseCertificate { role, .. }
            | DeploymentFailure::ParseExtension { role, .. } => Some(*role),
            DeploymentFailure::Tls { .. } => Some(MaterialRole::TlsKey),
            DeploymentFailure::OriginCa { .. } | DeploymentFailure::OriginKey { .. } => {
                Some(MaterialRole::OriginCa)
            }
            DeploymentFailure::VerifyLeaf { .. } => Some(MaterialRole::TlsLeaf),
            _ => None,
        }
    }

    /// Returns a finite rejection reason without exposing certificate or key bytes.
    #[must_use]
    pub fn certificate_rejection(&self) -> Option<CertificateRejection> {
        match &self.source {
            DeploymentFailure::Certificate { rejection, .. } => Some(*rejection),
            DeploymentFailure::ParseCertificate { .. }
            | DeploymentFailure::ParseExtension { .. } => Some(CertificateRejection::InvalidDer),
            DeploymentFailure::Tls { .. } | DeploymentFailure::OriginKey { .. } => {
                Some(CertificateRejection::InvalidIdentity)
            }
            DeploymentFailure::OriginCa { source } => Some(match source {
                crate::component::gateway::GatewayLoadError::TrustRootMismatch => {
                    CertificateRejection::TrustRootMismatch
                }
                crate::component::gateway::GatewayLoadError::OriginCa { .. } => {
                    CertificateRejection::InvalidIdentity
                }
            }),
            DeploymentFailure::VerifyLeaf { source } => Some(match source {
                rustls::Error::InvalidCertificate(certificate_error) => match certificate_error {
                    rustls::CertificateError::NotValidForName
                    | rustls::CertificateError::NotValidForNameContext { .. } => {
                        CertificateRejection::IpNotCovered
                    }
                    rustls::CertificateError::UnknownIssuer
                    | rustls::CertificateError::BadSignature => {
                        CertificateRejection::UntrustedIssuer
                    }
                    rustls::CertificateError::Expired
                    | rustls::CertificateError::ExpiredContext { .. }
                    | rustls::CertificateError::NotValidYet
                    | rustls::CertificateError::NotValidYetContext { .. } => {
                        CertificateRejection::LeafNotCurrentlyValid
                    }
                    rustls::CertificateError::InvalidPurpose
                    | rustls::CertificateError::InvalidPurposeContext { .. } => {
                        CertificateRejection::NotServerAuth
                    }
                    _ => CertificateRejection::ServerVerification,
                },
                _ => CertificateRejection::ServerVerification,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub(super) enum DeploymentFailure {
    #[snafu(display("deployment TLS identity loading failed"))]
    Tls { source: crate::tls::TlsError },
    #[snafu(display("deployment Origin CA loading failed"))]
    OriginCa {
        source: crate::component::gateway::GatewayLoadError,
    },
    #[snafu(display("deployment Origin CA key loading failed"))]
    OriginKey { source: rcgen::Error },
    #[snafu(display("deployment configuration failed"))]
    Configuration { source: crate::config::ConfigError },
    #[snafu(display("deployment certificate DER is invalid"))]
    ParseCertificate {
        role: MaterialRole,
        source: x509_parser::asn1_rs::Err<x509_parser::error::X509Error>,
    },
    #[snafu(display("deployment certificate extension is invalid"))]
    ParseExtension {
        role: MaterialRole,
        source: x509_parser::error::X509Error,
    },
    #[snafu(display("deployment certificate policy rejected material"))]
    Certificate {
        role: MaterialRole,
        rejection: CertificateRejection,
    },
    #[snafu(display("deployment TLS certificate verification failed"))]
    VerifyLeaf { source: rustls::Error },
    #[snafu(display("deployment TLS verifier construction failed"))]
    Verifier {
        source: rustls::client::VerifierBuilderError,
    },
    #[snafu(display("deployment requires at least one Server IP"))]
    EmptyServerIps,
    #[snafu(display("deployment validation time is unsupported"))]
    SystemTime { source: std::time::SystemTimeError },
    #[snafu(display("deployment validation time is unsupported"))]
    TimeInteger { source: std::num::TryFromIntError },
    #[snafu(display("deployment validation time is unsupported"))]
    TimeRange { source: time::error::ComponentRange },
    #[snafu(display("deployment Gateway expiry must be in the future"))]
    GatewayExpired,
    #[snafu(display("bootstrap database inspection failed"))]
    Database { source: crate::db::DatabaseError },
    #[snafu(context(false), display("bootstrap database inspection failed"))]
    Persistence { source: crate::db::PersistenceError },
    #[snafu(display("bootstrap operator inspection failed"))]
    Operator {
        source: crate::component::operator::OperatorError,
    },
}
