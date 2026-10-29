//! Typed terminal interactions shared by installation steps and scripted test fixtures.

pub mod notice;
pub mod terminal;

use notice::Notice;
use secrecy::SecretString;
use snafu::Snafu;

/// CA identity associated with a private-key retention decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaRole {
    /// Offline authority that signs Server TLS certificates.
    Control,
    /// Online authority that signs per-device Gateway certificates.
    Origin,
}

impl std::fmt::Display for CaRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Control => "Control CA",
            Self::Origin => "Local Origin CA",
        })
    }
}

/// Stable semantic identity for every prompt in the first-install flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptId {
    /// Collects all advertised Server IP addresses.
    ServerIps,
    /// Collects the unprivileged HTTPS listener port.
    HttpsPort,
    /// Collects the Gateway's DNS identity.
    GatewayHostname,
    /// Collects the upstream HTTPS origin.
    DomjudgeOrigin,
    /// Selects the local competition completion date.
    ContestEndDate,
    /// Collects the local competition completion time.
    ContestEndTime,
    /// Selects the local Gateway certificate expiry date.
    GatewayNotAfterDate,
    /// Collects the local Gateway certificate expiry time.
    GatewayNotAfterTime,
    /// Chooses how to obtain the Control trust anchor.
    ControlCaSource,
    /// Locates an existing Control certificate.
    ControlCaCertificatePath,
    /// Locates the offline signing key for import.
    ControlCaKeyPath,
    /// Unlocks an encrypted Control signing key.
    ControlCaKeyPassphrase,
    /// Chooses issuance, import, or offline signing.
    ServerTlsSource,
    /// Locates an existing Server certificate.
    ServerTlsCertificatePath,
    /// Locates the Server's existing private key.
    ServerTlsKeyPath,
    /// Unlocks an encrypted Server private key.
    ServerTlsKeyPassphrase,
    /// Locates the certificate returned by an offline signer.
    SignedCertificatePath,
    /// Chooses generation or import of the Origin authority.
    OriginCaSource,
    /// Locates an existing Origin certificate.
    OriginCaCertificatePath,
    /// Locates the corresponding Origin private key.
    OriginCaKeyPath,
    /// Unlocks an encrypted Origin private key.
    OriginCaKeyPassphrase,
    /// Protects a newly generated CA's export.
    NewCaPassphrase {
        /// Authority whose export is being protected.
        role: CaRole,
    },
    /// Chooses whether to retain the encrypted CA key on disk.
    KeyPersistence {
        /// Authority whose export may be retained.
        role: CaRole,
    },
    /// Requires explicit acknowledgement of external key retention.
    KeyRetentionConfirmation {
        /// Authority whose loss must be acknowledged.
        role: CaRole,
    },
    /// Authorizes the displayed file and service plan.
    PlanConfirmation,
    /// Chooses parameter editing or cancellation after declining the plan.
    PlanDeclined,
    /// Chooses whether to retry Server credential initialization.
    BootstrapRetry,
    /// Acknowledges host warnings before installation proceeds.
    ContinueAfterWarnings,
}

/// Prompt response including navigation without using sentinel input strings.
#[derive(Debug)]
pub enum Answer<T> {
    /// A successfully entered or selected value.
    Value(T),
    /// Escape requested a return to the preceding step.
    Back,
}

/// One displayed option with an explicit explanation when it cannot be selected.
#[derive(Debug, Clone)]
pub struct Choice<T> {
    /// Value supplied to the owning installation step.
    pub value: T,
    /// Human-readable English description.
    pub label: String,
    /// Reason the current deployment context prevents choosing this option.
    pub unavailable: Option<String>,
}

/// Terminal boundary; secret values must never be passed to logging or generic displays.
pub trait Interaction {
    /// Presents non-secret context directly to the controlling terminal.
    ///
    /// # Errors
    /// Returns a terminal output failure.
    fn show(&mut self, notice: &Notice) -> Result<(), InteractionError>;
    /// Chooses an available option while displaying reasons for unavailable alternatives.
    ///
    /// # Errors
    /// Returns interruption, invalid choices, or terminal failures.
    fn select<T: Clone>(
        &mut self,
        id: PromptId,
        prompt: &str,
        options: &[Choice<T>],
        default: usize,
    ) -> Result<Answer<T>, InteractionError>;
    /// Reads non-secret text with optional default and explanatory help.
    ///
    /// # Errors
    /// Returns interruption or terminal failures.
    fn text(
        &mut self,
        id: PromptId,
        prompt: &str,
        default: Option<&str>,
        help: Option<&str>,
    ) -> Result<Answer<String>, InteractionError>;
    /// Selects a calendar date, optionally excluding dates before the supplied minimum.
    ///
    /// # Errors
    /// Returns interruption, date conversion failures, or terminal failures.
    fn date(
        &mut self,
        id: PromptId,
        prompt: &str,
        default: time::Date,
        min: Option<time::Date>,
    ) -> Result<Answer<time::Date>, InteractionError>;
    /// Reads a yes/no decision with an explicit default.
    ///
    /// # Errors
    /// Returns interruption or terminal failures.
    fn confirm(
        &mut self,
        id: PromptId,
        prompt: &str,
        default: bool,
    ) -> Result<Answer<bool>, InteractionError>;
    /// Reads a nonempty hidden secret, repeating input when confirmation is requested.
    ///
    /// # Errors
    /// Returns interruption or terminal failures.
    fn secret(
        &mut self,
        id: PromptId,
        prompt: &str,
        confirm: bool,
    ) -> Result<Answer<SecretString>, InteractionError>;
    /// Requires a verbatim match to one of the accepted acknowledgement phrases.
    ///
    /// # Errors
    /// Returns interruption, an empty accepted set, or terminal failures.
    fn phrase(
        &mut self,
        id: PromptId,
        prompt: &str,
        accepted: &[&str],
    ) -> Result<Answer<String>, InteractionError>;
    /// Writes an encrypted private-key export exactly once between explicit markers.
    ///
    /// # Errors
    /// Returns a controlling-terminal write or flush failure.
    fn reveal_key(&mut self, role: CaRole, pem: &SecretString) -> Result<(), InteractionError>;
    /// Erases the terminal's visible screen and scrollback after secret export.
    ///
    /// # Errors
    /// Returns a controlling-terminal write or flush failure.
    fn clear_scrollback(&mut self) -> Result<(), InteractionError>;
}

/// Interaction failures with no stored answers or private material.
#[derive(Debug, Snafu)]
pub enum InteractionError {
    /// The administrator requested immediate cancellation through Ctrl-C.
    #[snafu(display("installation interrupted"))]
    Interrupted,
    /// Inquire could not read or render a prompt.
    #[snafu(display("terminal prompt failed"))]
    Terminal {
        /// Original prompt backend failure.
        source: inquire::InquireError,
    },
    /// Direct controlling-terminal output failed.
    #[snafu(display("cannot write to /dev/tty"))]
    Output {
        /// Original terminal I/O failure.
        source: std::io::Error,
    },
    /// A calendar date cannot be represented by the prompt backend.
    #[snafu(display("calendar date is outside the supported range"))]
    CalendarDateOutOfRange,
    /// A prompt backend calendar component cannot fit the application's component type.
    #[snafu(display("calendar component is outside the supported range"))]
    CalendarComponent {
        /// Original numeric conversion failure.
        source: std::num::TryFromIntError,
    },
    /// The prompt backend returned an unsupported calendar component.
    #[snafu(display("invalid calendar date"))]
    CalendarDate {
        /// Original calendar component validation failure.
        source: time::error::ComponentRange,
    },
    /// No available selection or accepted phrase was supplied.
    #[snafu(display("prompt has no available answers"))]
    NoChoices,
}
