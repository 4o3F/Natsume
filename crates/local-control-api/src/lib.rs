#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use zbus::zvariant::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SanitizedHardwareClaim {
    pub candidates: Vec<HardwareCandidate>,
    pub collection_complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct HardwareCandidate {
    pub anchor_kind: String,
    pub candidate_id: String,
    pub quality: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SessionTarget {
    pub session_instance_id: String,
    pub session_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LockSessionRequest {
    pub command_id: String,
    pub target: SessionTarget,
    pub requested_lock_epoch: u64,
    pub deadline_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UnlockSessionRequest {
    pub command_id: String,
    pub target: SessionTarget,
    pub expected_lock_epoch: u64,
    pub expected_lock_command_id: String,
    pub deadline_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TerminateSessionRequest {
    pub command_id: String,
    pub target: SessionTarget,
    pub deadline_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ApplyLockRequest {
    pub command_id: String,
    pub target: SessionTarget,
    pub lock_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ApplyUnlockRequest {
    pub command_id: String,
    pub target: SessionTarget,
    pub expected_lock_epoch: u64,
    pub expected_lock_command_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SessionControlApplied {
    pub command_id: String,
    pub target: SessionTarget,
    pub lock_epoch: u64,
    pub lock_state: SessionLockState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SessionChanged {
    pub previous: Option<SessionTarget>,
    pub current: Option<SessionTarget>,
    pub lock_epoch: u64,
    pub lock_state: SessionLockState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum GatewayBlockReason {
    Restoring,
    TransitionBlocked,
    SecretMissing,
    UpstreamUnhealthy,
    RecoveryRequired,
    Unassigned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum GatewayReasonCode {
    BootRestore,
    StateTransition,
    SecretNotInstalled,
    UpstreamProbeFailed,
    RecoveryJournal,
    NoAssignment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SuggestedAction {
    Wait,
    ContactOperator,
    CheckNetwork,
    RequestSecretSync,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
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

// Session lock implementations are desktop-only; this crate intentionally has no Caddy method.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum GraphicalSessionType {
    Wayland,
    X11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DisplayBackend {
    Wayland,
    X11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum UiPresentationState {
    Hidden,
    Presenting,
    PresentedFocused,
    PresentedUnfocused,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SeatInputPolicy {
    SeatCode,
    OperatorSelectedSeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SessionUiActionKind {
    ConfirmBinding,
    CancelBinding,
    Acknowledge,
    RetryLocalPresentation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SessionAgentLease {
    pub lease_id: String,
    pub target: SessionTarget,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UiTextParameter {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UiPresentationAck {
    pub target: SessionTarget,
    pub ui_revision: u64,
    pub presentation: UiPresentationState,
    pub display_backend: DisplayBackend,
    pub window_mapped: bool,
    pub focused: bool,
    pub stable_error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BindingSubmission {
    pub target: SessionTarget,
    pub prompt_command_id: String,
    pub prompt_nonce: String,
    pub seat_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
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

pub const DEVICE1_SERVICE: &str = "org.natsume.Device1";
pub const DEVICE1_PATH: &str = "/org/natsume/Device1";
pub const DEVICE1_INTERFACE: &str = "org.natsume.Device1";
pub const PRIVILEGED1_SERVICE: &str = "org.natsume.Privileged1";
pub const PRIVILEGED1_PATH: &str = "/org/natsume/Privileged1";
pub const PRIVILEGED1_INTERFACE: &str = "org.natsume.Privileged1";

pub const DEVICE1_INTROSPECTION_XML: &str = include_str!("../dbus/org.natsume.Device1.xml");
pub const PRIVILEGED1_INTROSPECTION_XML: &str = include_str!("../dbus/org.natsume.Privileged1.xml");

#[zbus::proxy(
    interface = "org.natsume.Device1",
    default_service = "org.natsume.Device1",
    default_path = "/org/natsume/Device1"
)]
pub trait Device1 {
    #[zbus(name = "RegisterSessionAgent")]
    fn register_session_agent(
        &self,
        registration: &SessionAgentRegistration,
    ) -> zbus::Result<(SessionAgentLease, SessionUiSnapshot)>;

    #[zbus(name = "RenewSessionAgentLease")]
    fn renew_session_agent_lease(
        &self,
        lease_id: &str,
        target: &SessionTarget,
    ) -> zbus::Result<SessionAgentLease>;

    #[zbus(name = "GetSessionUiSnapshot")]
    fn get_session_ui_snapshot(
        &self,
        target: &SessionTarget,
        after_revision: u64,
    ) -> zbus::Result<SessionUiSnapshot>;

    #[zbus(name = "SubmitSessionUiAction")]
    fn submit_session_ui_action(&self, action: &SessionUiAction) -> zbus::Result<()>;

    #[zbus(name = "SubmitBinding")]
    fn submit_binding(&self, submission: &BindingSubmission) -> zbus::Result<()>;

    #[zbus(name = "AcknowledgePresentation")]
    fn acknowledge_presentation(&self, acknowledgement: &UiPresentationAck) -> zbus::Result<()>;

    #[zbus(name = "UnregisterSessionAgent")]
    fn unregister_session_agent(
        &self,
        target: &SessionTarget,
        process_nonce: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal, name = "SessionUiSnapshotChanged")]
    fn session_ui_snapshot_changed(&self, snapshot: SessionUiSnapshot) -> zbus::Result<()>;

    #[zbus(signal, name = "SessionLeaseRevoked")]
    fn session_lease_revoked(&self, target: SessionTarget, reason: String) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.natsume.Privileged1",
    default_service = "org.natsume.Privileged1",
    default_path = "/org/natsume/Privileged1"
)]
pub trait Privileged1 {
    #[zbus(name = "CollectHardwareCandidates")]
    fn collect_hardware_candidates(&self) -> zbus::Result<SanitizedHardwareClaim>;

    #[zbus(name = "PrepareHomeInstance")]
    fn prepare_home_instance(
        &self,
        target: &SessionTarget,
        home_template_revision: &str,
    ) -> zbus::Result<()>;

    #[zbus(name = "ActivateHomeInstance")]
    fn activate_home_instance(&self, target: &SessionTarget) -> zbus::Result<()>;

    #[zbus(name = "RecoverHomeInstance")]
    fn recover_home_instance(
        &self,
        target: &SessionTarget,
        home_template_revision: &str,
    ) -> zbus::Result<()>;

    #[zbus(name = "GarbageCollectHomeInstance")]
    fn garbage_collect_home_instance(&self, retained_template_revision: &str) -> zbus::Result<()>;

    #[zbus(name = "QueryContestSession")]
    fn query_contest_session(&self) -> zbus::Result<Option<SessionTarget>>;

    #[zbus(name = "TerminateContestSession")]
    fn terminate_contest_session(&self, target: &SessionTarget) -> zbus::Result<()>;

    #[zbus(name = "RequestDesktopLock")]
    fn request_desktop_lock(
        &self,
        request: &ApplyLockRequest,
    ) -> zbus::Result<SessionControlApplied>;

    #[zbus(name = "RequestDesktopUnlock")]
    fn request_desktop_unlock(
        &self,
        request: &ApplyUnlockRequest,
    ) -> zbus::Result<SessionControlApplied>;

    #[zbus(name = "InstallManagedBrowserPolicy")]
    fn install_managed_browser_policy(&self, policy_revision: &str) -> zbus::Result<()>;
}
