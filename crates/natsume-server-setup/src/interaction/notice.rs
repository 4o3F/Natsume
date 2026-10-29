//! Non-secret installation notices rendered directly to the controlling terminal.

use super::{CaRole, Choice, PromptId};
use crate::preflight::PreflightWarning;
use crate::{
    config::DeploymentParameters,
    deployment::file::FileSpec,
    handoff::CertificateSummary,
    pki::rules::{ServerTlsOption, Unavailability},
    wizard::{CaSource, KeyRetention},
};
use std::net::SocketAddr;
use std::{fmt, path::PathBuf};

const CLOCK_NOT_SYNCHRONIZED: &str =
    "system time is not synchronized; check the clock before issuing certificates.";
const CLOCK_SYNCHRONIZATION_UNKNOWN: &str =
    "system time synchronization could not be determined; verify the clock.";
const EXISTING_DEPLOYMENT: &str = "This tool supports first installation only. Follow the operations manual for an existing deployment.";
const RESUME_REQUEST: &str = "Provide its signed Server certificate to resume.";
const SAVE_KEY: &str = "Save it securely; loss cannot be recovered.";
const SIGN_REQUEST: &str = "Sign it directly with all requested IP SANs and serverAuth, then rerun setup with the signed certificate.";
const BOOTSTRAP_PASSWORD_POLICY: &str = "Server bootstrap will ask for the initial operator login and password directly through /dev/tty. Follow its password policy; setup does not receive these credentials.";

/// Context displayed by the P3a terminal and system adapters.
#[derive(Debug, Clone)]
pub enum Notice {
    /// Host observations requiring acknowledgement before installation.
    PreflightWarnings {
        /// Nonfatal host conditions.
        warnings: Vec<PreflightWarning>,
    },
    /// Existing deployment files prevent first installation.
    ExistingDeployment {
        /// Occupied system paths.
        paths: Vec<PathBuf>,
    },
    /// Explains why a certificate source cannot currently be chosen.
    OptionUnavailable {
        /// Human-readable option name.
        label: String,
        /// Non-secret availability constraint.
        reason: String,
    },
    /// Allows an installation step to explain a rejected non-secret input.
    ValidationFailed {
        /// Sanitized explanation supplied by the owning step.
        message: String,
    },
    /// A previous offline signing request can be resumed.
    PendingRequestFound {
        /// Directory holding pending materials.
        path: PathBuf,
    },
    /// Introduces a one-time encrypted CA private-key export.
    KeyRevealIntro {
        /// Authority whose backup is being shown.
        role: CaRole,
    },
    /// Explains how to return an offline-signed Server certificate.
    OfflineSigningInstructions {
        /// Public signing request to take to the CA.
        csr_path: PathBuf,
        /// Issuer certificate expected on resume.
        control_ca_path: PathBuf,
    },
    /// Prepares the operator for the Server-owned credential prompt.
    BootstrapPasswordPolicy,
    /// Initialization already exists and must not be repeated.
    BootstrapSkipped,
    /// Administrator-only location used for new backups, pending requests, or handoff.
    PkiDirectory {
        /// Absolute path beneath the selected target root.
        path: PathBuf,
    },
    /// Complete non-secret plan presented before any publication.
    PlanSummary {
        /// Validated advertised deployment parameters.
        parameters: DeploymentParameters,
        /// Public certificate identities and their selected sources.
        certificates: Vec<PlannedCertificate>,
        /// Ordered paths with ownership and permission metadata.
        entries: Vec<FileSpec>,
        /// Whether this run only exports an offline request.
        pending_only: bool,
        /// Authorities whose encrypted exports may be saved after confirmation.
        generated: Vec<CaRole>,
    },
    /// Cleanup results and guidance after cancellation or failure.
    RollbackReport {
        /// Successfully deleted entries.
        removed: Vec<PathBuf>,
        /// Nonempty directories retained, including bootstrap outputs.
        kept: Vec<PathBuf>,
        /// Entries whose removal failed.
        failures: Vec<PathBuf>,
        /// Number of stop/disable failures.
        service_failures: usize,
        /// Whether an exposed key must be discarded.
        revoke_revealed: bool,
    },
    /// Installed endpoints, identities, retention choices, and public handoff result.
    InstallationSummary {
        /// Advertised HTTPS endpoints.
        addresses: Vec<SocketAddr>,
        /// Public Control CA identity.
        control_ca: CertificateSummary,
        /// Public Origin CA identity.
        origin_ca: CertificateSummary,
        /// Administrator's Control signing-key retention choice.
        control_key: KeyRetention,
        /// Administrator's Origin signing-key retention choice.
        origin_key: KeyRetention,
        /// Public export directory when publication succeeded.
        handoff_directory: Option<PathBuf>,
    },
}

impl fmt::Display for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PreflightWarnings { warnings } => {
                for warning in warnings {
                    let message = match warning {
                        PreflightWarning::ClockNotSynchronized => CLOCK_NOT_SYNCHRONIZED,
                        PreflightWarning::ClockSynchronizationUnknown => {
                            CLOCK_SYNCHRONIZATION_UNKNOWN
                        }
                    };
                    writeln!(f, "Warning: {message}")?;
                }
                Ok(())
            }
            Self::ExistingDeployment { paths } => {
                writeln!(f, "{EXISTING_DEPLOYMENT}")?;
                for path in paths {
                    writeln!(f, "  {}", path.to_string_lossy().escape_default())?;
                }
                Ok(())
            }
            Self::OptionUnavailable { label, reason } => {
                write!(f, "{label}: unavailable ({reason})")
            }
            Self::ValidationFailed { message } => write!(f, "Validation failed: {message}"),
            Self::PendingRequestFound { path } => {
                write!(
                    f,
                    "Pending offline signing request found at {}. ",
                    path.to_string_lossy().escape_default()
                )?;
                f.write_str(RESUME_REQUEST)
            }
            Self::KeyRevealIntro { role } => {
                write!(f, "The encrypted {role} private key will be shown once. ")?;
                f.write_str(SAVE_KEY)
            }
            Self::OfflineSigningInstructions {
                csr_path,
                control_ca_path,
            } => {
                write!(
                    f,
                    "Take {} to the offline CA corresponding to {}. ",
                    csr_path.to_string_lossy().escape_default(),
                    control_ca_path.to_string_lossy().escape_default()
                )?;
                f.write_str(SIGN_REQUEST)
            }
            Self::BootstrapPasswordPolicy => f.write_str(BOOTSTRAP_PASSWORD_POLICY),
            Self::BootstrapSkipped => f.write_str(
                "Server bootstrap is already initialized; skipping credential initialization.",
            ),
            Self::PkiDirectory { path } => write!(
                f,
                "Administrator PKI directory: {}",
                path.to_string_lossy().escape_default()
            ),
            Self::PlanSummary { .. } => self.format_plan(f),
            Self::RollbackReport {
                removed,
                kept,
                failures,
                service_failures,
                revoke_revealed,
            } => {
                writeln!(
                    f,
                    "Rollback removed={removed:?}; kept={kept:?}; failures={failures:?}; service failures={service_failures}"
                )?;
                if *revoke_revealed {
                    f.write_str(REVOKE_KEYS)?;
                }
                Ok(())
            }
            Self::InstallationSummary { .. } => self.format_installation(f),
        }
    }
}

const REVOKE_KEYS: &str = "Discard every private key revealed by this aborted installation attempt; generate fresh keys before retrying.";

/// Public certificate details used in the installation confirmation plan.
#[derive(Debug, Clone)]
pub struct PlannedCertificate {
    /// Material role displayed to the administrator.
    pub role: String,
    /// Generation, import, or offline-signing origin.
    pub source: String,
    /// Parsed X.509 subject name.
    pub subject: String,
    /// Fingerprint and validity interval.
    pub identity: CertificateSummary,
}

fn retention_label(retention: &KeyRetention) -> String {
    match retention {
        KeyRetention::Saved(path) => format!(
            "encrypted backup at {}",
            path.to_string_lossy().escape_default()
        ),
        KeyRetention::ExternalCopy => "saved separately by the administrator".into(),
        KeyRetention::Imported => "imported; original not copied".into(),
        KeyRetention::Offline => "held by the offline signer".into(),
    }
}

pub(crate) fn prompt(id: PromptId) -> &'static str {
    match id {
        PromptId::ServerIps => "Server IP addresses (comma separated)",
        PromptId::HttpsPort => "HTTPS port (1024-65535)",
        PromptId::GatewayHostname => "Gateway hostname",
        PromptId::DomjudgeOrigin => "DOMjudge HTTPS origin",
        PromptId::ContestEnd => "Contest end (RFC 3339 UTC Z)",
        PromptId::GatewayNotAfter => {
            "Gateway expiry (at least contest end + one day; RFC 3339 UTC Z)"
        }
        PromptId::ControlCaSource => "Control CA source",
        PromptId::ControlCaCertificatePath => "Control CA certificate path",
        PromptId::ControlCaKeyPath => "Control CA private key path",
        PromptId::ControlCaKeyPassphrase
        | PromptId::ServerTlsKeyPassphrase
        | PromptId::OriginCaKeyPassphrase => "Private key passphrase",
        PromptId::ServerTlsSource => "Server TLS source",
        PromptId::ServerTlsCertificatePath => "Server TLS certificate path",
        PromptId::ServerTlsKeyPath => "Server TLS private key path",
        PromptId::SignedCertificatePath => "Signed Server certificate path",
        PromptId::OriginCaSource => "Local Origin CA source",
        PromptId::OriginCaCertificatePath => "Origin CA certificate path",
        PromptId::OriginCaKeyPath => "Origin CA private key path",
        PromptId::NewCaPassphrase { .. } => "New CA export passphrase (confirm)",
        PromptId::KeyPersistence { .. } => {
            "Save this encrypted CA key to administrator PKI storage?"
        }
        PromptId::KeyRetentionConfirmation { .. } => {
            "Type 'key saved' to confirm a secure external copy, or 'abort' to discard this attempt"
        }
        PromptId::PlanConfirmation => "Apply this first-install plan?",
        PromptId::PlanDeclined => "Edit parameters or abort?",
        PromptId::BootstrapRetry => "Retry Server bootstrap?",
        PromptId::ContinueAfterWarnings => "Continue after reviewing these warnings?",
    }
}

pub(crate) fn control_choices() -> Vec<Choice<CaSource>> {
    [
        (CaSource::Generated, "Generate a new Control CA"),
        (
            CaSource::ImportedWithKey,
            "Import CA certificate and private key",
        ),
        (
            CaSource::CertificateOnly,
            "Import CA certificate only (offline key)",
        ),
    ]
    .into_iter()
    .map(|(value, label)| Choice {
        value,
        label: label.into(),
        unavailable: None,
    })
    .collect()
}

pub(crate) fn origin_choices() -> Vec<Choice<CaSource>> {
    [
        (CaSource::Generated, "Generate a new Local Origin CA"),
        (
            CaSource::ImportedWithKey,
            "Import Origin CA certificate and private key",
        ),
    ]
    .into_iter()
    .map(|(value, label)| Choice {
        value,
        label: label.into(),
        unavailable: None,
    })
    .collect()
}

pub(crate) fn tls_choice(
    value: ServerTlsOption,
    availability: Result<(), Unavailability>,
) -> Choice<ServerTlsOption> {
    let label = match value {
        ServerTlsOption::Issue => "Issue a new Server certificate",
        ServerTlsOption::Import => "Import Server certificate and private key",
        ServerTlsOption::SigningRequest => "Export a signing request for offline signing",
    };
    let unavailable = availability.err().map(|reason| {
        match reason {
            Unavailability::ControlKeyOffline => "Control CA private key is offline",
            Unavailability::NewCaHasNoExistingCertificates => {
                "A new CA has no existing certificates to import"
            }
            Unavailability::NewCaKeyAvailable => "The new CA key is available for local issuance",
        }
        .into()
    });
    Choice {
        value,
        label: label.into(),
        unavailable,
    }
}

pub(crate) const CONTROL_NAME: &str = "Natsume Control CA";
pub(crate) const ORIGIN_NAME: &str = "Natsume Local Origin CA";
pub(crate) const ORGANIZATION: &str = "Natsume";
pub(crate) const NOT_CA: &str = "The certificate must be a CA with keyCertSign usage.";
pub(crate) const KEY_MISMATCH: &str = "The private key does not match the certificate.";
pub(crate) const ORIGIN_P256: &str = "The Local Origin CA and private key must use ECDSA P-256.";
pub(crate) const INVALID_IP: &str = "Enter at least one usable IP address, separated by commas.";
pub(crate) const INVALID_PORT: &str = "Enter an HTTPS port from 1024 through 65535.";
pub(crate) const INVALID_TIME: &str = "Enter a valid RFC 3339 UTC timestamp ending in Z.";
pub(crate) const EDIT_LABEL: &str = "Edit parameters";
pub(crate) const EDIT_ORIGIN_LABEL: &str = "Review Local Origin CA";
pub(crate) const ABORT_LABEL: &str = "Abort";

pub(crate) fn source_label(source: CaSource) -> &'static str {
    match source {
        CaSource::Generated => "newly generated",
        CaSource::ImportedWithKey => "imported with private key; original not copied",
        CaSource::CertificateOnly => "imported certificate; signing key remains offline",
    }
}
pub(crate) const SERVER_TLS_NAME: &str = "Server TLS";
pub(crate) fn tls_source_label(source: ServerTlsOption) -> &'static str {
    match source {
        ServerTlsOption::Issue => "newly issued by the selected Control CA",
        ServerTlsOption::Import => "imported certificate and private key; original not copied",
        ServerTlsOption::SigningRequest => "signed offline; resumed from the pending request",
    }
}

impl Notice {
    fn format_plan(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::PlanSummary {
            parameters,
            certificates,
            entries,
            pending_only,
            generated,
        } = self
        else {
            unreachable!("notice formatter matches its variant")
        };

        writeln!(f, "First-install plan: {parameters:?}")?;
        for certificate in certificates {
            writeln!(
                f,
                "{}: {}; subject={}; SHA-256={}; valid {} through {}",
                certificate.role,
                certificate.source,
                certificate.subject.escape_default(),
                certificate.identity.fingerprint,
                certificate.identity.not_before,
                certificate.identity.not_after
            )?;
        }
        for entry in entries {
            writeln!(
                f,
                "{} uid={} gid={} mode={:04o}",
                entry.path.to_string_lossy().escape_default(),
                entry.owner.uid,
                entry.owner.gid,
                entry.mode
            )?;
        }
        if *pending_only {
            writeln!(
                f,
                "Only the pending request directory will be written. No system deployment paths will be written."
            )?;
        }
        for role in generated {
            writeln!(
                f,
                "{role}: encrypted private key will be revealed once; saving it to administrator PKI storage is optional and defaults to no."
            )?;
        }
        f.write_str("The database and server-root.key will never be modified or removed by setup.")
    }
    fn format_installation(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::InstallationSummary {
            addresses,
            control_ca,
            origin_ca,
            control_key,
            origin_key,
            handoff_directory,
        } = self
        else {
            unreachable!("notice formatter matches its variant")
        };

        for address in addresses {
            writeln!(f, "Server: https://{address}")?;
        }
        for (role, identity, key) in [
            (CaRole::Control, control_ca, control_key),
            (CaRole::Origin, origin_ca, origin_key),
        ] {
            writeln!(
                f,
                "{role}: SHA-256={}; valid {} through {}; private key: {}",
                identity.fingerprint,
                identity.not_before,
                identity.not_after,
                retention_label(key)
            )?;
        }
        if let Some(path) = handoff_directory {
            writeln!(
                f,
                "Client handoff: {}. Copy config.toml and both CA certificates to the Client and compare fingerprints.",
                path.to_string_lossy().escape_default()
            )?;
        } else {
            writeln!(
                f,
                "Client handoff export failed. Manually create Client config.toml with the first Server address and HTTPS port; copy control-ca.crt and local-origin-ca.crt from /etc/natsume/trust and verify both fingerprints."
            )?;
        }
        if !matches!(control_key, KeyRetention::Saved(_)) {
            f.write_str("The Control CA private key was not saved by setup. Future Server certificates require reimporting the signing key or offline CSR signing.")?;
        }
        Ok(())
    }
}

pub(crate) const CA_VALIDITY: &str =
    "The Control CA must be valid now and cover the Gateway expiry.";

/// Describes certificate policy classification without exposing internal enum formatting.
pub(crate) fn certificate_policy(
    role: Option<natsume_server::MaterialRole>,
    rejection: Option<natsume_server::CertificateRejection>,
) -> String {
    let role = role.map_or("Deployment materials", material_role);
    let reason = rejection.map_or("certificate validation failed", certificate_rejection);
    format!("{role}: {reason}")
}

fn material_role(role: natsume_server::MaterialRole) -> &'static str {
    use natsume_server::MaterialRole;
    match role {
        MaterialRole::ControlCa => "Control CA",
        MaterialRole::TlsLeaf => "Server TLS certificate",
        MaterialRole::TlsKey => "Server TLS private key",
        MaterialRole::OriginCa => "Local Origin CA",
        MaterialRole::LocalOriginRoot => "Local Origin trust anchor",
    }
}

fn certificate_rejection(rejection: natsume_server::CertificateRejection) -> &'static str {
    use natsume_server::CertificateRejection;
    match rejection {
        CertificateRejection::InvalidDer => "expected one complete DER certificate",
        CertificateRejection::InvalidIdentity => {
            "certificate and private key could not be loaded or paired"
        }
        CertificateRejection::NotCa => "certificate must have CA basic constraints",
        CertificateRejection::MissingKeyCertSign => {
            "CA must explicitly permit certificate signing (keyCertSign)"
        }
        CertificateRejection::NotP256 => "certificate and key must use ECDSA P-256",
        CertificateRejection::TrustRootMismatch => "online CA differs from the public trust anchor",
        CertificateRejection::IpNotCovered => {
            "certificate IP SANs must cover every configured Server IP"
        }
        CertificateRejection::UntrustedIssuer => {
            "certificate must be issued directly by the selected Control CA"
        }
        CertificateRejection::LeafNotCurrentlyValid => {
            "Server certificate is expired or not yet valid"
        }
        CertificateRejection::NotServerAuth => {
            "certificate must permit server authentication (serverAuth)"
        }
        CertificateRejection::ServerVerification => "Server TLS certificate verification failed",
        CertificateRejection::InsufficientValidity => {
            "certificate must remain valid through the Gateway expiry"
        }
        CertificateRejection::ExceedsControlCaValidity => {
            "Server certificate must not outlive the Control CA"
        }
        CertificateRejection::NotCurrentlyValid => "CA is expired or not yet valid",
        _ => "certificate validation failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use natsume_server::{CertificateRejection, MaterialRole};

    #[test]
    fn every_material_role_has_an_english_description() {
        for (role, expected) in [
            (MaterialRole::ControlCa, "Control CA"),
            (MaterialRole::TlsLeaf, "Server TLS certificate"),
            (MaterialRole::TlsKey, "Server TLS private key"),
            (MaterialRole::OriginCa, "Local Origin CA"),
            (MaterialRole::LocalOriginRoot, "Local Origin trust anchor"),
        ] {
            assert_eq!(
                certificate_policy(Some(role), None),
                format!("{expected}: certificate validation failed")
            );
        }
    }

    #[test]
    fn every_certificate_rejection_has_an_english_description() {
        for (reason, expected) in [
            (
                CertificateRejection::InvalidDer,
                "expected one complete DER certificate",
            ),
            (
                CertificateRejection::InvalidIdentity,
                "certificate and private key could not be loaded or paired",
            ),
            (
                CertificateRejection::NotCa,
                "certificate must have CA basic constraints",
            ),
            (
                CertificateRejection::MissingKeyCertSign,
                "CA must explicitly permit certificate signing (keyCertSign)",
            ),
            (
                CertificateRejection::NotP256,
                "certificate and key must use ECDSA P-256",
            ),
            (
                CertificateRejection::TrustRootMismatch,
                "online CA differs from the public trust anchor",
            ),
            (
                CertificateRejection::IpNotCovered,
                "certificate IP SANs must cover every configured Server IP",
            ),
            (
                CertificateRejection::UntrustedIssuer,
                "certificate must be issued directly by the selected Control CA",
            ),
            (
                CertificateRejection::LeafNotCurrentlyValid,
                "Server certificate is expired or not yet valid",
            ),
            (
                CertificateRejection::NotServerAuth,
                "certificate must permit server authentication (serverAuth)",
            ),
            (
                CertificateRejection::ServerVerification,
                "Server TLS certificate verification failed",
            ),
            (
                CertificateRejection::InsufficientValidity,
                "certificate must remain valid through the Gateway expiry",
            ),
            (
                CertificateRejection::ExceedsControlCaValidity,
                "Server certificate must not outlive the Control CA",
            ),
            (
                CertificateRejection::NotCurrentlyValid,
                "CA is expired or not yet valid",
            ),
        ] {
            assert_eq!(
                certificate_policy(None, Some(reason)),
                format!("Deployment materials: {expected}")
            );
        }
        assert_eq!(
            certificate_policy(None, None),
            "Deployment materials: certificate validation failed"
        );
    }
}
