//! Interactive first installation with explicit navigation and attempt-scoped rollback.

mod confirm;
mod entry;
mod finish;
mod params;
mod reveal;
mod secrets;
mod sources;
#[cfg(test)]
mod tests;

use crate::{
    config::{ConfigRenderError, DeploymentParameters},
    deployment::{
        apply::{ApplyError, ApplyReport, RollbackOutcome},
        file::{CreatedFile, FileError, Ownership},
        plan::{DeploymentPlan, Owners, PlanError},
    },
    handoff::{CertificateSummary, HandoffError, HandoffReport},
    health::HealthError,
    host::{Host, InputError, LocalTimeError},
    interaction::{Answer, CaRole, Interaction, InteractionError},
    pki::{
        Certificate, PrivateKey,
        generate::{GenerateError, SigningRequest},
        import::ImportError,
        pending::{PendingError, PendingRequest},
        rules::ServerTlsOption,
    },
    preflight::PreflightError,
    process::ProcessError,
    service::ServiceError,
};
use secrecy::SecretString;
use snafu::Snafu;
use std::{net::SocketAddr, path::PathBuf};

/// Filesystem root, administrator storage, and ownership used by one installation attempt.
#[derive(Debug, Clone)]
pub struct WizardContext {
    /// Root beneath which absolute deployment paths are resolved; production uses `/`.
    pub target_root: PathBuf,
    /// Absolute administrator PKI directory, resolved beneath the target root.
    pub pki_dir: PathBuf,
    /// Administrator identity for protected publication; service ownership comes from preflight.
    pub root_owner: Ownership,
}

/// Successful completion, an offline-signing pause, or explicit administrator cancellation.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "the requested outcome API owns the completed public summary directly"
)]
pub enum WizardOutcome {
    /// Deployment passed probes and startup health checks.
    Installed(#[doc = "Public installation and handoff results."] InstallationSummary),
    /// Only the administrator pending directory was published.
    PendingSignature {
        /// Public CSR to take to the offline signer.
        csr_path: PathBuf,
    },
    /// The administrator cancelled and this attempt's creations were rolled back.
    Aborted,
}

/// Disposition of a CA's original private key, without any key bytes or passphrases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRetention {
    /// An encrypted backup was explicitly published to administrator storage.
    Saved(#[doc = "Protected backup path."] PathBuf),
    /// The administrator confirmed a separately retained copy.
    ExternalCopy,
    /// An imported key was used in memory without copying its original.
    Imported,
    /// The Control key remains with an offline signer.
    Offline,
}

/// Non-secret results of an installed Server and its Client handoff.
#[derive(Debug)]
pub struct InstallationSummary {
    /// Advertised HTTPS endpoints in administrator-specified order.
    pub addresses: Vec<SocketAddr>,
    /// Control CA fingerprint and validity interval.
    pub control_ca: CertificateSummary,
    /// Local Origin CA fingerprint and validity interval.
    pub origin_ca: CertificateSummary,
    /// Retention of the offline signing key.
    pub control_key: KeyRetention,
    /// Retention of the original online authority key.
    pub origin_key: KeyRetention,
    /// Public handoff export or its original nonfatal failure.
    pub handoff: Result<HandoffReport, HandoffError>,
    /// Final summary display or its original nonfatal terminal failure.
    pub summary_display: Result<(), InteractionError>,
    /// Pending request cleanup or its original nonfatal filesystem failure.
    pub pending_cleanup: Result<(), PendingError>,
}

/// Results of reversing an attempt, including best-effort service cleanup failures.
#[derive(Debug, Default)]
pub struct WizardRollback {
    /// Filesystem removal, retention, and failure records for deployment and PKI entries.
    pub files: RollbackOutcome,
    /// Failed stop or disable operations, retaining their original sources.
    pub service_failures: Vec<ServiceError>,
}

/// Original installation failure and the outcome of any required rollback.
#[derive(Debug, Snafu)]
pub enum WizardError {
    /// Existing deployment files prevent first installation without any writes.
    #[snafu(
        context(suffix(Wizard)),
        display("existing deployment; follow the operations manual")
    )]
    AlreadyDeployed,
    /// An installation step failed, with its original cause and any cleanup results.
    #[snafu(display("installation failed: {source}"))]
    Failed {
        /// Original typed cause; input secrets are never stored in errors.
        source: WizardFailure,
        /// Cleanup results when this attempt had begun publishing files.
        rollback: Option<Box<WizardRollback>>,
    },
}

/// Typed failures from the wizard's system, interaction, and publication boundaries.
#[derive(Debug, Snafu)]
pub enum WizardFailure {
    /// Escape navigation, consumed by the state machine.
    #[snafu(display("return to previous step"))]
    Back,
    /// Explicit cancellation, consumed by the state machine.
    #[snafu(display("installation cancelled"))]
    Cancelled,
    /// Deployment files already exist; setup only supports first installation.
    #[snafu(display("existing deployment; follow the operations manual"))]
    AlreadyDeployed,
    /// A terminal interaction failed.
    #[snafu(context(false), display("terminal interaction failed"))]
    Interaction {
        /// Original terminal cause, without answers.
        source: InteractionError,
    },
    /// Host prerequisites failed.
    #[snafu(context(false), display("host preflight failed"))]
    Preflight {
        /// Original prerequisite failure.
        source: PreflightError,
    },
    /// An administrator input could not be read.
    #[snafu(context(false), display("input file could not be read"))]
    Input {
        /// Original bounded-read failure.
        source: InputError,
    },
    /// A certificate or private-key input was rejected.
    #[snafu(context(false), display("PKI import failed"))]
    Import {
        /// Original parser or decryption failure, without input bytes.
        source: ImportError,
    },
    /// Certificate generation or encrypted export failed.
    #[snafu(context(false), display("PKI generation failed"))]
    Generate {
        /// Original cryptographic failure.
        source: GenerateError,
    },
    /// A host timezone conversion failed while preparing a plan summary.
    #[snafu(context(false), display("local time conversion failed"))]
    LocalTime {
        /// Original timezone conversion failure.
        source: LocalTimeError,
    },
    /// Formatting a selected UTC timestamp failed.
    #[snafu(context(false), display("cannot format UTC deployment timestamp"))]
    TimeFormat {
        /// Original timestamp formatting failure.
        source: time::error::Format,
    },
    /// Parameter or configuration validation failed.
    #[snafu(context(false), display("configuration validation failed"))]
    Config {
        /// Original safe validation failure.
        source: ConfigRenderError,
    },
    /// Pending request publication, loading, or cleanup failed.
    #[snafu(context(false), display("offline signing request operation failed"))]
    Pending {
        /// Original pending operation failure.
        source: PendingError,
    },
    /// The deployment plan could not be built.
    #[snafu(context(false), display("deployment planning failed"))]
    Plan {
        /// Original planning failure.
        source: PlanError,
    },
    /// Publication failed and already reversed its partial deployment writes.
    #[snafu(context(false), display("deployment publication failed"))]
    Apply {
        /// Original publication failure including its own rollback result.
        #[snafu(source(from(ApplyError, Box::new)))]
        source: Box<ApplyError>,
    },
    /// An administrator PKI file could not be published.
    #[snafu(context(false), display("administrator PKI publication failed"))]
    File {
        /// Original safe filesystem operation failure.
        source: FileError,
    },
    /// A service-user child could not complete normally.
    #[snafu(context(false), display("service-user process failed"))]
    Process {
        /// Original process cause.
        source: ProcessError,
    },
    /// A read-only probe returned a rejected status.
    #[snafu(display("Server check as the natsume-server user failed: {}", status.description()))]
    Probe {
        /// Probe protocol result, without process output.
        status: crate::probe::ProbeExit,
    },
    /// Credential initialization exhausted its bounded retries.
    #[snafu(display(
        "Server bootstrap did not create the administrator account (exit status {code}); see the Server's message above"
    ))]
    Bootstrap {
        /// Last nonzero Server exit code before retries stopped.
        code: i32,
    },
    /// A systemd operation failed.
    #[snafu(context(false), display("service operation failed"))]
    Service {
        /// Original systemctl cause.
        source: ServiceError,
    },
    /// Startup never passed the pinned TLS check.
    #[snafu(context(false), display("Server health check failed"))]
    Health {
        /// Final health failure.
        source: HealthError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Parameters(params::Field),
    SignedCertificate,
    ControlCa,
    ServerTls,
    OriginCa,
    PkiDirectory,
    Passphrase(CaRole),
    Validate,
    Confirm,
    Reveal,
    Apply,
    Bootstrap,
    Start,
    Finish,
}

/// Origin of certificate material selected by the installation wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaSource {
    /// Newly generated authority certificate and private key.
    Generated,
    /// Existing authority imported with its private key.
    ImportedWithKey,
    /// Existing Control authority whose signing key remains offline.
    CertificateOnly,
}

#[derive(Debug)]
struct CaMaterial {
    certificate: Certificate,
    key: Option<PrivateKey>,
    source: CaSource,
    passphrase: Option<SecretString>,
    retention: KeyRetention,
}

#[derive(Debug)]
enum TlsMaterial {
    Issue,
    Identity {
        certificate: Certificate,
        key: PrivateKey,
    },
    Request {
        key: PrivateKey,
        csr: SigningRequest,
    },
}

#[derive(Debug, Default)]
struct Answers {
    draft: params::Draft,
    params: Option<DeploymentParameters>,
    control: Option<CaMaterial>,
    tls: Option<TlsMaterial>,
    tls_source: Option<ServerTlsOption>,
    origin: Option<CaMaterial>,
    plan: Option<DeploymentPlan>,
}

impl Answers {
    fn parameters(&self) -> &DeploymentParameters {
        let Some(parameters) = &self.params else {
            unreachable!("parameters precede PKI selection")
        };
        parameters
    }
    fn control_ca(&self) -> &CaMaterial {
        let Some(control) = &self.control else {
            unreachable!("Control CA precedes TLS selection")
        };
        control
    }
    fn origin_ca(&self) -> &CaMaterial {
        let Some(origin) = &self.origin else {
            unreachable!("Origin CA precedes deployment validation")
        };
        origin
    }
    fn deployment_plan(&self) -> &DeploymentPlan {
        let Some(plan) = &self.plan else {
            unreachable!("deployment validation precedes publication")
        };
        plan
    }
}

impl CaMaterial {
    fn private_key(&self) -> &PrivateKey {
        let Some(key) = &self.key else {
            unreachable!("this source requires an available CA key")
        };
        key
    }
}

struct Wizard<'a, I, H> {
    interaction: &'a mut I,
    host: &'a mut H,
    context: &'a WizardContext,
    service_owner: Option<Ownership>,
    answers: Answers,
    pending: Option<PendingRequest>,
    applied: Option<ApplyReport>,
    pki_files: Vec<CreatedFile>,
    pki_directories: Vec<PathBuf>,
    enabled: bool,
    revealed: bool,
}

impl<I: Interaction, H: Host> Wizard<'_, I, H> {
    fn check_interrupted(&self) -> Result<(), WizardFailure> {
        if self.host.interrupted() {
            return Err(InteractionError::Interrupted.into());
        }
        Ok(())
    }
}

fn value<T>(answer: Answer<T>) -> Result<T, WizardFailure> {
    match answer {
        Answer::Value(value) => Ok(value),
        Answer::Back => BackSnafu.fail(),
    }
}

/// Runs first installation, exporting pending requests or reversing this attempt on failure.
///
/// # Errors
/// Returns pre-commit failures with cleanup results; failures after health checks remain installed.
/// Operation errors take precedence over simultaneous cancellation. An [`ApplyError`]'s completed
/// rollback is transferred into the returned cleanup report and displayed once, without retrying it.
pub fn run<I: Interaction, H: Host>(
    interaction: &mut I,
    host: &mut H,
    context: &WizardContext,
) -> Result<WizardOutcome, WizardError> {
    let mut wizard = Wizard {
        interaction,
        host,
        context,
        service_owner: None,
        answers: Answers::default(),
        pending: None,
        applied: None,
        pki_files: Vec::new(),
        pki_directories: Vec::new(),
        enabled: false,
        revealed: false,
    };
    match wizard.execute() {
        Ok(outcome) => Ok(outcome),
        Err(mut source) => {
            let rollback = wizard.cleanup(&mut source);
            if matches!(
                source,
                WizardFailure::Cancelled
                    | WizardFailure::Back
                    | WizardFailure::Interaction {
                        source: InteractionError::Interrupted
                    }
            ) {
                Ok(WizardOutcome::Aborted)
            } else if matches!(source, WizardFailure::AlreadyDeployed) {
                Err(WizardError::AlreadyDeployed)
            } else {
                Err(WizardError::Failed {
                    source,
                    rollback: rollback.map(Box::new),
                })
            }
        }
    }
}
