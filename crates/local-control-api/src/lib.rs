#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SanitizedHardwareClaim {
    pub candidates: Vec<HardwareCandidate>,
    pub collection_complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareCandidate {
    pub anchor_kind: String,
    pub candidate_id: String,
    pub quality: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupIdentityState {
    CleanFirstStart,
    Matched,
    Indeterminate,
    IdentityUnavailable,
    IdentityRecordMissingOrCorrupt,
    SiteNamespaceMismatch,
    ResetRequired,
    VaultCorrupt,
    EnrollmentPending,
    Enrolled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLockState {
    None,
    Locking,
    Locked,
    Unlocking,
    Unlocked,
    Terminating,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTarget {
    pub session_instance_id: String,
    pub session_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockSessionRequest {
    pub command_id: String,
    pub target: SessionTarget,
    pub requested_lock_epoch: u64,
    pub deadline_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnlockSessionRequest {
    pub command_id: String,
    pub target: SessionTarget,
    pub expected_lock_epoch: u64,
    pub expected_lock_command_id: String,
    pub deadline_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminateSessionRequest {
    pub command_id: String,
    pub target: SessionTarget,
    pub deadline_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyLockRequest {
    pub command_id: String,
    pub target: SessionTarget,
    pub lock_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyUnlockRequest {
    pub command_id: String,
    pub target: SessionTarget,
    pub expected_lock_epoch: u64,
    pub expected_lock_command_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionControlApplied {
    pub command_id: String,
    pub target: SessionTarget,
    pub lock_epoch: u64,
    pub lock_state: SessionLockState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionChanged {
    pub previous: Option<SessionTarget>,
    pub current: Option<SessionTarget>,
    pub lock_epoch: u64,
    pub lock_state: SessionLockState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayBlockReason {
    Restoring,
    TransitionBlocked,
    SecretMissing,
    UpstreamUnhealthy,
    RecoveryRequired,
    Unassigned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayReasonCode {
    BootRestore,
    StateTransition,
    SecretNotInstalled,
    UpstreamProbeFailed,
    RecoveryJournal,
    NoAssignment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestedAction {
    Wait,
    ContactOperator,
    CheckNetwork,
    RequestSecretSync,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayStatusSnapshot {
    pub schema_version: u32,
    pub state: GatewayBlockReason,
    pub reason_code: GatewayReasonCode,
    pub updated_at_unix_ms: i64,
    pub machine_short_id: String,
    pub seat_label: Option<String>,
    pub operation_short_id: Option<String>,
    pub progress_current: Option<u32>,
    pub progress_total: Option<u32>,
    pub suggested_action: SuggestedAction,
}

// Frozen zbus interface traits belong here after method signatures are finalized.
// Session lock implementations are desktop-only; this crate intentionally has no Caddy method.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphicalSessionType {
    Wayland,
    X11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayBackend {
    Wayland,
    X11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiPresentationState {
    Hidden,
    Presenting,
    PresentedFocused,
    PresentedUnfocused,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionScreenKind {
    Hidden,
    IdleStatus,
    BindingPrompt,
    BindingPending,
    BindingResult,
    RecoveryStatus,
    LockPresentation,
    FatalLocalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatInputPolicy {
    SeatCode,
    OperatorSelectedSeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionUiActionKind {
    ConfirmBinding,
    CancelBinding,
    Acknowledge,
    RetryLocalPresentation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct SessionAgentCapabilities {
    pub graphical_session_type: GraphicalSessionType,
    pub display_backend: DisplayBackend,
    pub notifications_available: bool,
    pub desktop_lock_supported: bool,
    pub desktop_unlock_supported: bool,
    pub ime_supported: bool,
    pub hidpi_supported: bool,
    pub multi_monitor_supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAgentRegistration {
    pub session_instance_id: String,
    pub logind_session_id: String,
    pub contest_uid: u32,
    pub seat_id: String,
    pub boot_id: String,
    pub process_nonce: String,
    pub agent_version: String,
    pub capabilities: SessionAgentCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAgentLease {
    pub lease_id: String,
    pub target: SessionTarget,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiTextParameter {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionUiSnapshot {
    pub schema_version: u32,
    pub target: SessionTarget,
    pub ui_revision: u64,
    pub screen: SessionScreenKind,
    pub message_id: String,
    pub parameters: Vec<UiTextParameter>,
    pub machine_short_id: String,
    pub seat_label: Option<String>,
    pub prompt_command_id: Option<String>,
    pub prompt_nonce: Option<String>,
    pub expires_at_unix_ms: Option<i64>,
    pub seat_input_policy: Option<SeatInputPolicy>,
    pub presentation: UiPresentationState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiPresentationAck {
    pub target: SessionTarget,
    pub ui_revision: u64,
    pub presentation: UiPresentationState,
    pub display_backend: DisplayBackend,
    pub window_mapped: bool,
    pub focused: bool,
    pub stable_error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingSubmission {
    pub target: SessionTarget,
    pub prompt_command_id: String,
    pub prompt_nonce: String,
    pub seat_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionUiAction {
    pub target: SessionTarget,
    pub ui_revision: u64,
    pub action: SessionUiActionKind,
}

/// Exact basename used by both the system-wide entry and the managed-Home shadow guard.
pub const SESSION_AGENT_AUTOSTART_BASENAME: &str = "org.natsume.SessionAgent.desktop";

/// Only this relative path may be removed or rejected by the managed-Home preflight.
pub const SESSION_AGENT_USER_AUTOSTART_RELATIVE_PATH: &str =
    ".config/autostart/org.natsume.SessionAgent.desktop";

/// Exact owner-only singleton path below the current graphical session `XDG_RUNTIME_DIR`.
pub const SESSION_AGENT_SINGLETON_RELATIVE_PATH: &str = "natsume/session-agent.lock";

/// The sole supported product invocation. Display/session parameters are never accepted.
pub const SESSION_AGENT_AUTOSTART_MODE: &str = "--autostart";
