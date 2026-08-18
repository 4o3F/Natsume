use serde_json::value::RawValue;
use snafu::Snafu;
use uuid::Uuid;

use natsume_device_protocol::is_canonical_command_id;

use crate::application::device::DeviceId;

pub(crate) const COMMAND_REQUEST_FINGERPRINT_DOMAIN: &[u8] = b"natsume:command-request:v1";
pub(crate) const REQUEST_FINGERPRINT_VERSION: i32 = 1;

pub(crate) struct CommandId(Uuid);

impl CommandId {
    pub(crate) fn parse(value: &str) -> Result<Self, CommandError> {
        parse_canonical_uuid_v7(value)
            .map(Self)
            .map_err(|()| CommandError::CommandIdInvalid)
    }

    pub(crate) const fn value(&self) -> Uuid {
        self.0
    }
}

pub(crate) struct CommandRequestInput {
    pub(crate) device_id: String,
    pub(crate) kind: String,
    pub(crate) payload_version: i32,
    pub(crate) payload: Box<RawValue>,
    pub(crate) reason_code: Option<String>,
    pub(crate) group_correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandRequestFingerprint {
    pub(crate) version: i32,
    pub(crate) sha256: Vec<u8>,
}

pub(crate) struct ValidatedCommandRequest {
    pub(crate) device_id: DeviceId,
    pub(crate) kind: CommandKind,
    pub(crate) payload_version: i32,
    pub(crate) frozen_payload_json: String,
    pub(crate) fingerprint: CommandRequestFingerprint,
    pub(crate) group_correlation_id: Option<String>,
}

pub(crate) trait DeviceCommandDispatchNotifier: Send + Sync {
    fn notify_command_dispatch(&self, device_pk: &str);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandOutcome {
    Created,
    Replayed,
}

pub(crate) struct DispatchableCommand {
    pub(crate) command_id: String,
    pub(crate) kind: CommandKind,
    pub(crate) payload_version: i32,
    pub(crate) frozen_payload_json: String,
    pub(crate) created_at: String,
    pub(crate) deadline_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandStatusWriteOutcome {
    UpdatedNonterminal,
    UpdatedTerminal,
    IgnoredTransition,
    IgnoredRegression,
    IgnoredUnknownCommand,
    IgnoredForeignCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportedCommandState {
    Received,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Expired,
    ManualInterventionRequired,
}

impl ReportedCommandState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
            Self::ManualInterventionRequired => "manual_intervention_required",
        }
    }

    pub(crate) const fn is_terminal(self) -> bool {
        match self {
            Self::Received | Self::Running => false,
            Self::Succeeded
            | Self::Failed
            | Self::Cancelled
            | Self::Expired
            | Self::ManualInterventionRequired => true,
        }
    }
}

/// The persisted lifecycle of a Command row.
///
/// Terminal states deliberately share one stage rather than an ordinal each: a derived
/// ordering would rank `failed` above `succeeded` and let a later report overwrite a terminal
/// row, which [`contracts`] §6 forbids.
///
/// [`contracts`]: ../../../docs/contracts.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandLifecycleState {
    Created,
    Received,
    Running,
    Terminal(ReportedCommandState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransitionDecision {
    Apply,
    DuplicateNoop,
    Regression,
}

impl CommandLifecycleState {
    pub(crate) fn parse_persisted(value: &str) -> Result<Self, CommandError> {
        match value {
            "created" => Ok(Self::Created),
            "received" => Ok(Self::Received),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Terminal(ReportedCommandState::Succeeded)),
            "failed" => Ok(Self::Terminal(ReportedCommandState::Failed)),
            "cancelled" => Ok(Self::Terminal(ReportedCommandState::Cancelled)),
            "expired" => Ok(Self::Terminal(ReportedCommandState::Expired)),
            "manual_intervention_required" => Ok(Self::Terminal(
                ReportedCommandState::ManualInterventionRequired,
            )),
            _ => Err(CommandError::PersistenceFailed),
        }
    }

    /// Classifies how a Device-reported state relates to this row.
    pub(crate) const fn classify(self, reported: ReportedCommandState) -> TransitionDecision {
        match (self, reported) {
            (Self::Received, ReportedCommandState::Received)
            | (Self::Running, ReportedCommandState::Running)
            | (Self::Terminal(ReportedCommandState::Succeeded), ReportedCommandState::Succeeded)
            | (Self::Terminal(ReportedCommandState::Failed), ReportedCommandState::Failed)
            | (Self::Terminal(ReportedCommandState::Cancelled), ReportedCommandState::Cancelled)
            | (Self::Terminal(ReportedCommandState::Expired), ReportedCommandState::Expired)
            | (
                Self::Terminal(ReportedCommandState::ManualInterventionRequired),
                ReportedCommandState::ManualInterventionRequired,
            ) => TransitionDecision::DuplicateNoop,
            (Self::Created | Self::Received, _) => TransitionDecision::Apply,
            (Self::Running, reported) if reported.is_terminal() => TransitionDecision::Apply,
            (Self::Running | Self::Terminal(_), _) => TransitionDecision::Regression,
        }
    }
}

pub(crate) struct CommandStatusWrite {
    pub(crate) command_id: Uuid,
    pub(crate) state: ReportedCommandState,
    pub(crate) terminal_error_code: Option<String>,
}

pub(crate) struct CommandLifecycleFacts {
    pub(crate) device_pk: DeviceId,
    pub(crate) kind: CommandKind,
    pub(crate) state: CommandLifecycleState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandKind {
    SyncState,
    SyncSecret,
    OpenBindingPrompt,
    LockSession,
    UnlockSession,
    TerminateSession,
    ResetHome,
}

impl CommandKind {
    pub(super) fn parse_request(value: &str) -> Result<Self, CommandError> {
        Self::from_text(value).ok_or(CommandError::KindInvalid)
    }

    pub(crate) fn parse_persisted(value: &str) -> Result<Self, CommandError> {
        Self::from_text(value).ok_or(CommandError::PersistenceFailed)
    }

    fn from_text(value: &str) -> Option<Self> {
        match value {
            "sync_state" => Some(Self::SyncState),
            "sync_secret" => Some(Self::SyncSecret),
            "open_binding_prompt" => Some(Self::OpenBindingPrompt),
            "lock_session" => Some(Self::LockSession),
            "unlock_session" => Some(Self::UnlockSession),
            "terminate_session" => Some(Self::TerminateSession),
            "reset_home" => Some(Self::ResetHome),
            _ => None,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SyncState => "sync_state",
            Self::SyncSecret => "sync_secret",
            Self::OpenBindingPrompt => "open_binding_prompt",
            Self::LockSession => "lock_session",
            Self::UnlockSession => "unlock_session",
            Self::TerminateSession => "terminate_session",
            Self::ResetHome => "reset_home",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum CommandError {
    #[snafu(display("the Command ID is invalid"))]
    CommandIdInvalid,
    #[snafu(display("the Command request body is invalid"))]
    RequestInvalid,
    #[snafu(display("the Device ID is invalid"))]
    DeviceIdInvalid,
    #[snafu(display("the Command kind is invalid"))]
    KindInvalid,
    #[snafu(display("the Command payload is invalid"))]
    PayloadInvalid,
    #[snafu(display("the Command reason code is invalid"))]
    ReasonCodeInvalid,
    #[snafu(display("the Command group correlation ID is invalid"))]
    GroupCorrelationIdInvalid,
    #[snafu(display("the Device does not exist"))]
    DeviceNotFound,
    #[snafu(display("the Command request conflicts with persisted facts"))]
    RequestConflict,
    #[snafu(display("the Command request could not be canonicalized"))]
    CanonicalizationFailed,
    #[snafu(display("Command persistence failed"))]
    PersistenceFailed,
}

pub(super) fn parse_canonical_uuid_v7(value: &str) -> Result<Uuid, ()> {
    if !is_canonical_command_id(value) {
        return Err(());
    }
    Uuid::parse_str(value).map_err(|_| ())
}
