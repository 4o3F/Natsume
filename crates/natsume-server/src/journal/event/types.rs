use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use super::contract::{BoundError, JournalField, sealed};
use crate::component::device::{DeviceId, EnrollmentReviewId};

impl sealed::Sealed for DeviceId {}
impl JournalField for DeviceId {}
impl Serialize for DeviceId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_text())
    }
}
impl<'de> Deserialize<'de> for DeviceId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| serde::de::Error::custom("invalid journal device ID"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(transparent)]
pub(crate) struct RequestId(
    /// Request correlation UUID supplied explicitly by the producer.
    Uuid,
);
impl sealed::Sealed for RequestId {}
impl JournalField for RequestId {}
impl RequestId {
    pub(crate) const fn from_request(id: Uuid) -> Self {
        Self(id)
    }
    pub(crate) const fn value(self) -> Uuid {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(transparent)]
pub(crate) struct OperationId(
    /// Operation correlation UUID supplied explicitly by the producer.
    Uuid,
);
impl sealed::Sealed for OperationId {}
impl JournalField for OperationId {}
impl OperationId {
    pub(crate) const fn from_operation(id: Uuid) -> Self {
        Self(id)
    }
    pub(crate) const fn value(self) -> Uuid {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(transparent)]
pub(crate) struct SessionId(
    /// Lease session UUID supplied explicitly by the producer.
    Uuid,
);
impl sealed::Sealed for SessionId {}
impl JournalField for SessionId {}
impl SessionId {
    pub(crate) const fn from_lease(id: Uuid) -> Self {
        Self(id)
    }
    pub(crate) const fn value(self) -> Uuid {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, specta::Type)]
pub(crate) struct ReviewId(
    /// Connection-local review identity supplied by the enrollment component.
    #[specta(type = String)]
    EnrollmentReviewId,
);
impl sealed::Sealed for ReviewId {}
impl JournalField for ReviewId {}
impl ReviewId {
    pub(crate) const fn from_enrollment_review(id: EnrollmentReviewId) -> Self {
        Self(id)
    }
}
impl Serialize for ReviewId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.as_text())
    }
}
impl<'de> Deserialize<'de> for ReviewId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        EnrollmentReviewId::parse(&value)
            .map(Self::from_enrollment_review)
            .ok_or_else(|| serde::de::Error::custom("invalid journal review ID"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(transparent)]
pub(crate) struct OperatorId(Uuid);
impl sealed::Sealed for OperatorId {}
impl JournalField for OperatorId {}
impl OperatorId {
    pub(crate) const fn value(&self) -> Uuid {
        self.0
    }
    pub(crate) const fn from_authenticated_operator(id: Uuid) -> Self {
        Self(id)
    }
    pub(crate) const fn from_route_target(id: Uuid) -> Self {
        Self(id)
    }
    pub(crate) const fn from_committed_operator(id: Uuid) -> Self {
        Self(id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(transparent)]
pub(crate) struct SeatCode(String);
impl sealed::Sealed for SeatCode {}
impl JournalField for SeatCode {}
impl SeatCode {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn from_committed_binding(value: &str) -> Result<Self, BoundError> {
        Self::from_persisted(value.to_owned())
    }

    fn from_persisted(value: String) -> Result<Self, BoundError> {
        if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
            return Err(BoundError::SeatCode);
        }
        Ok(Self(value))
    }
}
impl<'de> Deserialize<'de> for SeatCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_persisted(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(transparent)]
pub(crate) struct ErrorToken(String);
impl sealed::Sealed for ErrorToken {}
impl JournalField for ErrorToken {}
impl ErrorToken {
    pub(crate) fn from_static_api_code(value: &'static str) -> Result<Self, BoundError> {
        Self::from_persisted(value.to_owned())
    }

    fn from_persisted(value: String) -> Result<Self, BoundError> {
        if value.is_empty()
            || value.len() > 64
            || !value.as_bytes()[0].is_ascii_uppercase()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(BoundError::ErrorToken);
        }
        Ok(Self(value))
    }
}
impl<'de> Deserialize<'de> for ErrorToken {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_persisted(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(transparent)]
pub(crate) struct AuditCause(
    /// Validated static API failure discriminant, stored without response text.
    String,
);
impl sealed::Sealed for AuditCause {}
impl JournalField for AuditCause {}
impl AuditCause {
    pub(crate) fn from_static_api_cause(value: &'static str) -> Result<Self, BoundError> {
        Self::from_persisted(value.to_owned())
    }
    fn from_persisted(value: String) -> Result<Self, BoundError> {
        if value.is_empty()
            || value.len() > 64
            || !value.as_bytes()[0].is_ascii_lowercase()
            || !value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(BoundError::AuditCause);
        }
        Ok(Self(value))
    }
}
impl<'de> Deserialize<'de> for AuditCause {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_persisted(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Category {
    Device,
    Audit,
    Auth,
    Journal,
}
impl sealed::Sealed for Category {}
impl JournalField for Category {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Severity {
    Info,
    Warning,
    Error,
}
impl sealed::Sealed for Severity {}
impl JournalField for Severity {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AdmissionStage {
    ServerStopping,
    Capacity,
    ProtocolVersion,
    Challenge,
    Proof,
    Metadata,
    ResumeAuthority,
    Enrollment,
    Activation,
    ReadyExchange,
    Attach,
}
impl sealed::Sealed for AdmissionStage {}
impl JournalField for AdmissionStage {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DisconnectReason {
    Disabled,
    Revoked,
    ControlKeyReplaced,
    PingSilence,
    ClientClose,
    TransportError,
    ProtocolViolation,
    ActorInboxUnavailable,
    SendFailed,
    SendTimeout,
    OutgoingClosed,
    ClientFault,
    ServerFault,
    TransientLimit,
    OutboundBackpressure,
    OutboundClosed,
    ServerShutdown,
    ActorFailed,
}
impl sealed::Sealed for DisconnectReason {}
impl JournalField for DisconnectReason {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActualResource {
    Gateway,
    BindingAccess,
    RuntimeConfig,
    Foreground,
    Session,
    Home,
    Power,
}
impl sealed::Sealed for ActualResource {}
impl JournalField for ActualResource {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActualState {
    Unknown,
    None,
    Starting,
    Running,
    Terminating,
    Ambiguous,
    Error,
    Waiting,
    Contest,
    Greeter,
    Other,
    Steady,
    Resetting,
    RecoveryRequired,
    Idle,
    Pending,
    Accepted,
    Expired,
    Absent,
    Applied,
    Failed,
    Blocked,
    Restoring,
    Ready,
    UpstreamUnhealthy,
}
impl sealed::Sealed for ActualState {}
impl JournalField for ActualState {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewCancelCause {
    ClientClose,
    Timeout,
    Replaced,
    ServerStopping,
    TransportError,
}
impl sealed::Sealed for ReviewCancelCause {}
impl JournalField for ReviewCancelCause {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LifecycleState {
    Enabled,
    Disabled,
    Revoked,
}
impl sealed::Sealed for LifecycleState {}
impl JournalField for LifecycleState {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BindingCode {
    NotFound,
    Unmapped,
    Occupied,
}
impl sealed::Sealed for BindingCode {}
impl JournalField for BindingCode {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TargetAction {
    SetForeground,
    TerminateSession,
    ResetHome,
    PowerOff,
}
impl sealed::Sealed for TargetAction {}
impl JournalField for TargetAction {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TargetResult {
    Accepted,
    DeviceNotFound,
    DeviceNotEnabled,
    InvalidDeviceState,
    InvalidTarget,
    EpochExhausted,
}
impl sealed::Sealed for TargetResult {}
impl JournalField for TargetResult {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuditRoute {
    Session,
    Device,
    DeviceBinding,
    ReviewApprove,
    ReviewDeny,
    Imports,
    Import,
    ImportCommit,
    OperatorAccount,
    PasswordResetIssue,
    Invitations,
    Invitation,
    InvitationRegenerate,
    PasswordChange,
    PasswordReset,
    PasswordResetInspect,
    Registration,
    RegistrationInspect,
    PrintRequeue,
    Printers,
    PrinterBinding,
    PrinterPairingCode,
    PrintSources,
    PrintSource,
    PhotoSources,
    PhotoPairingCode,
    PhotoSourceState,
    Provisioning,
    TargetSubmission,
}
impl sealed::Sealed for AuditRoute {}
impl JournalField for AuditRoute {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HttpMethod {
    Post,
    Put,
    Patch,
    Delete,
    Options,
    Connect,
    Trace,
    Other,
}
impl sealed::Sealed for HttpMethod {}
impl JournalField for HttpMethod {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuditTarget {
    Device(DeviceId),
    Operator(OperatorId),
    Binding(Uuid),
    Seat(Uuid),
    Account(Uuid),
    Invitation(Uuid),
    Review(ReviewId),
    Import(Uuid),
    PrintSource(Uuid),
    PhotoSource(Uuid),
    Printer(Uuid),
    PrintExecution(#[specta(type = specta_typescript::Number)] i64),
    PasswordReset(Uuid),
}
impl sealed::Sealed for AuditTarget {}
impl JournalField for AuditTarget {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuthFailureClass {
    InvalidCredentials,
    RateLimited,
    Infrastructure,
}
impl sealed::Sealed for AuthFailureClass {}
impl JournalField for AuthFailureClass {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PauseReason {
    DiskBudget,
}
impl sealed::Sealed for PauseReason {}
impl JournalField for PauseReason {}

impl Category {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Audit => "audit",
            Self::Auth => "auth",
            Self::Journal => "journal",
        }
    }
}
impl Severity {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}
