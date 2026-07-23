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
