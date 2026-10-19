//! Non-secret, boot-local diagnostic evidence. Never an authority or a liveness proof.

use serde::{Deserialize, Serialize};

use crate::{HardwareIdentitySources, MachineIdentityQuality};

pub const DIAGNOSTIC_FORMAT_VERSION: u32 = 1;
pub const DIAGNOSTIC_DIRECTORY: &str = "/run/natsume-diagnostics";
pub const DIAGNOSTIC_REPORT_NAME: &str = "latest.json";
pub const DIAGNOSTIC_FAILURE_NAME: &str = "last-failure.json";
pub const MAX_DIAGNOSTIC_REPORT_BYTES: usize = 16 * 1024;
pub const MAX_DIAGNOSTIC_EVENTS: usize = 12;

/// The owning Daemon's last observed phase, not its current process state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStage {
    Starting,
    SystemBus,
    Helper,
    IdentityPreflight,
    HardwareIdentity,
    LocalGateway,
    ControlIdentity,
    Reconciliation,
    ControlConfiguration,
    Connecting,
    Handshake,
    PendingReview,
    AuthorityInstallation,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStatus {
    Starting,
    Retrying,
    WaitingForReview,
    Denied,
    Active,
    Stopped,
}

/// Reviewed classifications for this local evidence boundary only.
/// They do not select business retries or authorize any operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiagnosticCode {
    SystemBusUnavailable,
    HelperUnavailable,
    SiteConfigurationInvalid,
    IdentityArtifactScanFailed,
    IdentityRecordMissingOrCorrupt,
    IdentityInsufficientSources,
    IdentityUnsupported,
    IdentityUnavailable,
    IdentityMismatch,
    IdentityPersistenceFailed,
    LocalGatewayFailed,
    ControlKeyInvalid,
    ControlKeyEntropyUnavailable,
    ControlKeyEncodingFailed,
    ControlKeyPersistenceFailed,
    ControlManifestInvalid,
    ControlManifestSerializationFailed,
    ControlManifestPersistenceFailed,
    ReconciliationFailed,
    EndpointConfigurationInvalid,
    TrustRootInvalid,
    TlsConfigurationFailed,
    ReconnectEntropyUnavailable,
    LocalDeactivationFailed,
    PresentationCacheFailed,
    AuthorityPersistenceFailed,
    ConnectTimeout,
    TcpConnectionRefused,
    TcpNetworkUnreachable,
    TcpConnectionFailed,
    TlsCertificateInvalid,
    TlsHandshakeFailed,
    WebsocketUpgradeFailed,
    WebsocketSubprotocolMismatch,
    WebsocketConnectionFailed,
    HandshakeTimeout,
    HandshakeDisconnected,
    HandshakeInvalidMessage,
    HandshakeWriteFailed,
    EnrollmentDenied,
    LocalAddressUnavailable,
    ActiveConnectionEnded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityRecordDiagnostic {
    NotChecked,
    Absent,
    Valid,
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityComparisonDiagnostic {
    NotChecked,
    CleanFirstStart,
    Matched,
    Indeterminate,
    Mismatched,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityDiagnostic {
    pub record: IdentityRecordDiagnostic,
    pub comparison: IdentityComparisonDiagnostic,
    pub sources: Option<HardwareIdentitySources>,
    pub valid_source_count: Option<u8>,
    pub machine_hardware_id: Option<String>,
    pub quality: Option<MachineIdentityQuality>,
}

impl Default for IdentityDiagnostic {
    fn default() -> Self {
        Self {
            record: IdentityRecordDiagnostic::NotChecked,
            comparison: IdentityComparisonDiagnostic::NotChecked,
            sources: None,
            valid_source_count: None,
            machine_hardware_id: None,
            quality: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticConnectionPurpose {
    Enrollment,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticHandshakeStep {
    AwaitingChallenge,
    ChallengeReceived,
    ProofSent,
    PendingReview,
    Denied,
    AuthorityReceived,
    AuthorityInstalled,
    ReadySent,
    SessionReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticRouteState {
    #[default]
    Unknown,
    MatchingObservedRoute,
    NoMatchingObservedRoute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLinkState {
    #[default]
    Unknown,
    Up,
    Down,
}

/// Kernel /proc route-entry evidence, not a route lookup or connectivity proof.
/// Table membership is not established; policy/source-specific routing may differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct NetworkDiagnostic {
    pub sampled_at_boot_ms: Option<u64>,
    pub route: DiagnosticRouteState,
    pub interface: Option<String>,
    pub next_hop: Option<String>,
    pub carrier: DiagnosticLinkState,
    pub administratively_up: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ConnectionDiagnostic {
    pub endpoint_ip: Option<String>,
    pub endpoint_port: Option<u16>,
    pub trust_root_sha256: Option<String>,
    pub network: NetworkDiagnostic,
    pub attempt: u64,
    pub attempt_duration_ms: Option<u64>,
    pub retry_delay_ms: Option<u64>,
    pub local_ip: Option<String>,
    pub http_status: Option<u16>,
    pub purpose: Option<DiagnosticConnectionPurpose>,
    pub handshake_step: Option<DiagnosticHandshakeStep>,
    pub peer_error_code: Option<String>,
    pub device_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticFailure {
    pub code: DiagnosticCode,
    pub first_at_boot_ms: Option<u64>,
    pub last_at_boot_ms: Option<u64>,
    pub consecutive_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticEvent {
    pub at_boot_ms: Option<u64>,
    pub stage: DiagnosticStage,
    pub status: DiagnosticStatus,
    pub code: Option<DiagnosticCode>,
}

/// A historical report, even when recently written. Consumers must obtain
/// liveness separately; neither status nor retry delay remains true after exit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticReport {
    pub format_version: u32,
    pub boot_id: Option<String>,
    pub daemon_instance_id: String,
    pub daemon_pid: u32,
    pub daemon_version: String,
    pub control_protocol: String,
    pub sequence: u64,
    pub captured_at_unix_ms: Option<u64>,
    pub captured_at_boot_ms: Option<u64>,
    pub stage: DiagnosticStage,
    pub last_completed_stage: Option<DiagnosticStage>,
    pub status: DiagnosticStatus,
    pub failure: Option<DiagnosticFailure>,
    pub identity: IdentityDiagnostic,
    pub connection: ConnectionDiagnostic,
    pub events: Vec<DiagnosticEvent>,
    pub events_truncated: bool,
}
