#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use zbus::zvariant::Type;

/// Aggregate quality of one Helper-derived Machine Hardware ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MachineIdentityQuality {
    Medium,
    Strong,
}

/// Complete successful machine identity decision returned by the root helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DerivedMachineIdentity {
    pub machine_hardware_id: String,
    pub quality: MachineIdentityQuality,
}

/// Closed failure classification for Helper-owned machine identity derivation.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.natsume.Privileged1.Error", impl_display = true)]
pub enum MachineIdentityError {
    #[zbus(error)]
    ZBus(zbus::Error),
    InvalidArguments(String),
    InsufficientSources(String),
    Unsupported(String),
}

/// Exact logind graphical-session identity captured within one boot.
///
/// Both fields must match before a privileged session effect is applied. A
/// replacement session is therefore never selected for an older target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct GraphicalSession {
    pub logind_session_id: String,
    pub boot_id: String,
}

/// Exact lock level requested for the current graphical session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SessionLockLevel {
    Unlocked,
    Locked,
}

/// Bounded observation of the fixed contestant user's eligible graphical session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ContestSessionState {
    None,
    Active,
    Locked,
    Ambiguous,
}

/// Re-sampled contestant session state returned by the privileged helper.
///
/// `session` is present exactly for `Active` and `Locked`. Ambiguous or absent
/// sessions intentionally expose no candidate target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ContestSessionObservation {
    pub state: ContestSessionState,
    pub session: Option<GraphicalSession>,
}

/// Screen selected by the Daemon for the current graphical session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SessionScreenKind {
    Hidden,
    BindingPrompt,
    BindingPending,
}

/// Complete Session Agent presentation for one exact graphical session.
///
/// A Binding prompt is actionable only when both `negotiation_id` and
/// `submission_epoch` are present. These values come from the current Binding
/// intent; there is no Prompt Command or prompt nonce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SessionUiSnapshot {
    pub session: GraphicalSession,
    pub ui_revision: u64,
    pub screen: SessionScreenKind,
    pub binding_error_code: Option<String>,
    pub negotiation_id: Option<String>,
    pub submission_epoch: Option<u64>,
}

/// Short-lived ownership lease for the registered Session Agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SessionAgentLease {
    pub lease_id: String,
    pub session: GraphicalSession,
    pub expires_at_unix_ms: i64,
}

/// A user-confirmed Binding input for the exact current negotiation generation.
///
/// Repeating this value is a transport replay; only a newly displayed
/// `submission_epoch` represents a new user submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BindingSubmission {
    pub session: GraphicalSession,
    pub negotiation_id: String,
    pub submission_epoch: u64,
    pub seat_code: String,
}

/// Durable helper-side phase of one fixed contestant Home reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum HomeResetPhase {
    Prepared,
    Applied,
    Verified,
    RecoveryRequired,
}

/// Re-sampled helper-side progress for one Home reset epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct HomeResetProgress {
    pub reset_epoch: u64,
    pub phase: HomeResetPhase,
}

/// Owner-only singleton path below the active graphical session's runtime directory.
pub const SESSION_AGENT_SINGLETON_RELATIVE_PATH: &str = "natsume/session-agent.lock";

pub const DEVICE1_SERVICE: &str = "org.natsume.Device1";
pub const DEVICE1_PATH: &str = "/org/natsume/Device1";
pub const PRIVILEGED1_SERVICE: &str = "org.natsume.Privileged1";
pub const PRIVILEGED1_PATH: &str = "/org/natsume/Privileged1";

/// Typed Session Agent to Device Daemon IPC.
#[zbus::proxy(
    interface = "org.natsume.Device1",
    default_service = "org.natsume.Device1",
    default_path = "/org/natsume/Device1"
)]
pub trait Device1 {
    #[zbus(name = "RegisterSessionAgent")]
    fn register_session_agent(
        &self,
        session: &GraphicalSession,
    ) -> zbus::Result<(SessionAgentLease, SessionUiSnapshot)>;

    #[zbus(name = "RenewSessionAgentLease")]
    fn renew_session_agent_lease(
        &self,
        lease_id: &str,
        session: &GraphicalSession,
    ) -> zbus::Result<SessionAgentLease>;

    #[zbus(name = "GetSessionUiSnapshot")]
    fn get_session_ui_snapshot(
        &self,
        lease_id: &str,
        session: &GraphicalSession,
    ) -> zbus::Result<SessionUiSnapshot>;

    #[zbus(name = "SubmitBinding")]
    fn submit_binding(&self, lease_id: &str, submission: &BindingSubmission) -> zbus::Result<()>;
}

/// Closed root capabilities available only to the Device Daemon.
#[zbus::proxy(
    interface = "org.natsume.Privileged1",
    default_service = "org.natsume.Privileged1",
    default_path = "/org/natsume/Privileged1"
)]
pub trait Privileged1 {
    #[zbus(name = "DeriveMachineIdentity")]
    fn derive_machine_identity(
        &self,
        fleet_namespace_uuid: &str,
    ) -> Result<DerivedMachineIdentity, MachineIdentityError>;

    #[zbus(name = "HasHomeResetState")]
    fn has_home_reset_state(&self) -> zbus::Result<bool>;

    #[zbus(name = "QueryContestSession")]
    fn query_contest_session(&self) -> zbus::Result<ContestSessionObservation>;

    #[zbus(name = "SetContestSessionLock")]
    fn set_contest_session_lock(
        &self,
        session: &GraphicalSession,
        level: SessionLockLevel,
    ) -> zbus::Result<()>;

    #[zbus(name = "TerminateContestSession")]
    fn terminate_contest_session(&self, session: &GraphicalSession) -> zbus::Result<()>;

    #[zbus(name = "PrepareHomeReset")]
    fn prepare_home_reset(&self, reset_epoch: u64) -> zbus::Result<()>;

    #[zbus(name = "QueryHomeReset")]
    fn query_home_reset(&self) -> zbus::Result<Option<HomeResetProgress>>;

    #[zbus(name = "ApplyHomeReset")]
    fn apply_home_reset(&self, reset_epoch: u64) -> zbus::Result<()>;

    #[zbus(name = "VerifyHomeReset")]
    fn verify_home_reset(&self, reset_epoch: u64) -> zbus::Result<HomeResetProgress>;

    #[zbus(name = "RecoverHomeReset")]
    fn recover_home_reset(&self, reset_epoch: u64) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_control_types_have_stable_dbus_signatures() {
        assert_eq!(<SessionUiSnapshot as Type>::SIGNATURE, "((ss)tuasasat)");
        assert_eq!(<ContestSessionObservation as Type>::SIGNATURE, "(ua(ss))");
        assert_eq!(<Option<HomeResetProgress> as Type>::SIGNATURE, "a(tu)");
    }
}
