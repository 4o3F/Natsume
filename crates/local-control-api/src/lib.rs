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
    InsufficientSources(String),
    Unsupported(String),
}

/// Closed failure classification for fixed Home and Session capabilities.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.natsume.Privileged1.Error", impl_display = true)]
pub enum ResourceControlError {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The operation may be retried after rechecking its durable progress and guards.
    Unavailable(String),
    /// A safety precondition or persisted state must be repaired before applying effects.
    Rejected(String),
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

/// The only two roles accepted by privileged graphical-session operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SessionRole {
    Waiting,
    Contest,
}

/// Lifecycle of one fixed role, independent of foreground and GNOME locking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum GraphicalSessionState {
    None,
    Starting,
    Running,
    Terminating,
    Ambiguous,
    Error,
}

/// Actual seat0 foreground; these observation values are not remote targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SessionForeground {
    Unknown,
    Waiting,
    Contest,
    Greeter,
    Other,
    None,
}

/// Helper-owned observation of one role. Display evidence is always fresh.
///
/// Only Starting/Running/Terminating may carry an exact session identity.
/// A Running lifecycle does not imply desktop readiness or foreground ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct GraphicalSessionObservation {
    pub state: GraphicalSessionState,
    pub session: Option<GraphicalSession>,
    pub desktop_ready: bool,
    pub locked_hint: bool,
}

/// The helper owns OS observations; the Daemon separately owns Agent UI leases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ManagedSessionsObservation {
    pub waiting: GraphicalSessionObservation,
    pub contest: GraphicalSessionObservation,
    pub foreground: SessionForeground,
}

/// Screen selected by the Daemon for the current graphical session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SessionScreenKind {
    Waiting,
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

/// Frame evidence for one exact UI revision and graphical session.
/// A false frame flag with zero dimensions withdraws presentation while retaining
/// the authenticated lease, for example during a monitor resize.
///
/// Device1 authenticates the caller connection and lease separately. A first
/// frame with nonzero fullscreen dimensions is necessary, not sufficient, for
/// `waiting_ready`: fresh Helper desktop/session observations are also required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SessionPresentation {
    pub session: GraphicalSession,
    pub ui_revision: u64,
    pub first_frame_presented: bool,
    pub fullscreen_width: u32,
    pub fullscreen_height: u32,
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
    Draining,
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

    #[zbus(name = "ConfirmSessionPresentation")]
    fn confirm_session_presentation(
        &self,
        lease_id: &str,
        presentation: &SessionPresentation,
    ) -> zbus::Result<()>;

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
    fn derive_machine_identity(&self) -> Result<DerivedMachineIdentity, MachineIdentityError>;

    #[zbus(name = "HasHomeResetState")]
    fn has_home_reset_state(&self) -> Result<bool, ResourceControlError>;

    #[zbus(name = "QueryManagedSessions")]
    fn query_managed_sessions(&self) -> Result<ManagedSessionsObservation, ResourceControlError>;

    /// Completes local boot preparation before the first business foreground.
    #[zbus(name = "PrepareBootSessions")]
    fn prepare_boot_sessions(
        &self,
        waiting: &GraphicalSession,
    ) -> Result<bool, ResourceControlError>;

    /// Starts or observes the fixed GDM preparation service; never replaces a live role.
    #[zbus(name = "PrepareSession")]
    fn prepare_session(
        &self,
        role: SessionRole,
    ) -> Result<GraphicalSessionObservation, ResourceControlError>;

    /// After sustained presentation failure, spends at most one captured waiting
    /// recovery attempt per boot. Replays resume that capture, never a replacement.
    /// True means its fixed GDM preparation still needs observation.
    #[zbus(name = "RecoverWaitingSession")]
    fn recover_waiting_session(
        &self,
        expected: &Option<GraphicalSession>,
    ) -> Result<bool, ResourceControlError>;

    /// Resumes only an existing captured waiting attempt; never spends a new budget.
    #[zbus(name = "ResumeWaitingRecovery")]
    fn resume_waiting_recovery(&self) -> Result<bool, ResourceControlError>;

    #[zbus(name = "ActivateSession")]
    fn activate_session(
        &self,
        role: SessionRole,
        session: &GraphicalSession,
    ) -> Result<(), ResourceControlError>;

    /// Withdraws contest login permission and cancels the fixed pending login.
    #[zbus(name = "CloseContestAdmission")]
    fn close_contest_admission(&self) -> Result<(), ResourceControlError>;

    #[zbus(name = "TerminateContestSession")]
    fn terminate_contest_session(
        &self,
        session: &GraphicalSession,
    ) -> Result<(), ResourceControlError>;

    #[zbus(name = "RecoverLocalHome")]
    fn recover_local_home(&self) -> Result<(), ResourceControlError>;

    #[zbus(name = "IsHomeReady")]
    fn is_home_ready(&self) -> Result<bool, ResourceControlError>;

    #[zbus(name = "PrepareHomeReset")]
    fn prepare_home_reset(
        &self,
        reset_epoch: u64,
        waiting: &GraphicalSession,
    ) -> Result<(), ResourceControlError>;

    #[zbus(name = "QueryHomeReset")]
    fn query_home_reset(&self) -> Result<Option<HomeResetProgress>, ResourceControlError>;

    #[zbus(name = "ApplyHomeReset")]
    fn apply_home_reset(&self, reset_epoch: u64) -> Result<(), ResourceControlError>;

    #[zbus(name = "VerifyHomeReset")]
    fn verify_home_reset(
        &self,
        reset_epoch: u64,
    ) -> Result<HomeResetProgress, ResourceControlError>;

    #[zbus(name = "RecoverHomeReset")]
    fn recover_home_reset(&self, reset_epoch: u64) -> Result<(), ResourceControlError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_control_types_have_stable_dbus_signatures() {
        assert_eq!(<SessionUiSnapshot as Type>::SIGNATURE, "((ss)tuasasat)");
        assert_eq!(
            <GraphicalSessionObservation as Type>::SIGNATURE,
            "(ua(ss)bb)"
        );
        assert_eq!(
            <ManagedSessionsObservation as Type>::SIGNATURE,
            "((ua(ss)bb)(ua(ss)bb)u)"
        );
        assert_eq!(<SessionPresentation as Type>::SIGNATURE, "((ss)tbuu)");
        assert_eq!(<Option<HomeResetProgress> as Type>::SIGNATURE, "a(tu)");
    }
}
