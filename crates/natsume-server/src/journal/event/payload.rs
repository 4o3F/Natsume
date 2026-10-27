use std::net::IpAddr;

use serde::Deserialize;
use uuid::Uuid;

use super::{
    contract::{Bounded, DetailsWriter, JournalEvent, sealed},
    types::{
        ActualResource, ActualState, AdmissionStage, AuditRoute, AuditTarget, AuthFailureClass,
        BindingCode, Category, DisconnectReason, ErrorToken, HttpMethod, LifecycleState,
        PauseReason, ReviewCancelCause, SeatCode, Severity, TargetAction, TargetResult,
    },
};
use crate::component::device::DeviceId;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdmissionRejected {
    pub(crate) stage: AdmissionStage,
    pub(crate) peer_ip: IpAddr,
    pub(crate) device_id: Option<DeviceId>,
}

impl sealed::Sealed for AdmissionRejected {}

impl JournalEvent for AdmissionRejected {
    const KIND: &'static str = "admission_rejected";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Warning;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("stage", &self.stage);
        w.field("peer_ip", &self.peer_ip);
        w.field("device_id", &self.device_id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LeaseAttached {
    pub(crate) session_id: Uuid,
}

impl sealed::Sealed for LeaseAttached {}

impl JournalEvent for LeaseAttached {
    const KIND: &'static str = "lease_attached";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("session_id", &self.session_id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LeaseReplaced {
    pub(crate) old: Uuid,
    pub(crate) new: Uuid,
}

impl sealed::Sealed for LeaseReplaced {}

impl JournalEvent for LeaseReplaced {
    const KIND: &'static str = "lease_replaced";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("old", &self.old);
        w.field("new", &self.new);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Disconnected {
    pub(crate) session_id: Uuid,
    pub(crate) reason: DisconnectReason,
}

impl sealed::Sealed for Disconnected {}

impl JournalEvent for Disconnected {
    const KIND: &'static str = "disconnected";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Warning;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("session_id", &self.session_id);
        w.field("reason", &self.reason);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReconcileRetry {
    pub(crate) attempt: u32,
}

impl sealed::Sealed for ReconcileRetry {}

impl JournalEvent for ReconcileRetry {
    const KIND: &'static str = "reconcile_retry";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Warning;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("attempt", &self.attempt);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActorStopped {
    pub(crate) panicked: bool,
    pub(crate) session_id: Option<Uuid>,
}

impl sealed::Sealed for ActorStopped {}

impl JournalEvent for ActorStopped {
    const KIND: &'static str = "actor_stopped";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Error;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("panicked", &self.panicked);
        w.field("session_id", &self.session_id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActualChanged {
    pub(crate) resource: ActualResource,
    pub(crate) from: ActualState,
    pub(crate) to: ActualState,
}

impl sealed::Sealed for ActualChanged {}

impl JournalEvent for ActualChanged {
    const KIND: &'static str = "actual_changed";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("resource", &self.resource);
        w.field("from", &self.from);
        w.field("to", &self.to);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewCreated {}

impl sealed::Sealed for ReviewCreated {}

impl JournalEvent for ReviewCreated {
    const KIND: &'static str = "review_created";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, _w: &mut DetailsWriter) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewCancelled {
    pub(crate) cause: ReviewCancelCause,
}

impl sealed::Sealed for ReviewCancelled {}

impl JournalEvent for ReviewCancelled {
    const KIND: &'static str = "review_cancelled";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("cause", &self.cause);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnrollmentApproved {
    pub(crate) automatic: bool,
}

impl sealed::Sealed for EnrollmentApproved {}

impl JournalEvent for EnrollmentApproved {
    const KIND: &'static str = "enrollment_approved";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("automatic", &self.automatic);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnrollmentDenied {}

impl sealed::Sealed for EnrollmentDenied {}

impl JournalEvent for EnrollmentDenied {
    const KIND: &'static str = "enrollment_denied";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Warning;

    fn write(&self, _w: &mut DetailsWriter) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivationCommitted {}

impl sealed::Sealed for ActivationCommitted {}

impl JournalEvent for ActivationCommitted {
    const KIND: &'static str = "activation_committed";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, _w: &mut DetailsWriter) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdmissionCompleted {}

impl sealed::Sealed for AdmissionCompleted {}

impl JournalEvent for AdmissionCompleted {
    const KIND: &'static str = "admission_completed";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, _w: &mut DetailsWriter) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LifecycleChanged {
    pub(crate) to: LifecycleState,
}

impl sealed::Sealed for LifecycleChanged {}

impl JournalEvent for LifecycleChanged {
    const KIND: &'static str = "lifecycle_changed";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("to", &self.to);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BindingBound {
    pub(crate) binding_id: Uuid,
    pub(crate) seat_code: SeatCode,
}

impl sealed::Sealed for BindingBound {}

impl JournalEvent for BindingBound {
    const KIND: &'static str = "binding_bound";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("binding_id", &self.binding_id);
        w.field("seat_code", &self.seat_code);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BindingRejected {
    pub(crate) code: BindingCode,
}

impl sealed::Sealed for BindingRejected {}

impl JournalEvent for BindingRejected {
    const KIND: &'static str = "binding_rejected";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Warning;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("code", &self.code);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BindingUnbound {
    pub(crate) seat_code: SeatCode,
}

impl sealed::Sealed for BindingUnbound {}

impl JournalEvent for BindingUnbound {
    const KIND: &'static str = "binding_unbound";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("seat_code", &self.seat_code);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TargetSubmitted {
    pub(crate) operation_id: Uuid,
    pub(crate) action: TargetAction,
    pub(crate) result: TargetResult,
    pub(crate) replayed: bool,
}

impl sealed::Sealed for TargetSubmitted {}

impl JournalEvent for TargetSubmitted {
    const KIND: &'static str = "target_submitted";
    const CATEGORY: Category = Category::Device;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("operation_id", &self.operation_id);
        w.field("action", &self.action);
        w.field("result", &self.result);
        w.field("replayed", &self.replayed);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorRequest {
    pub(crate) route: AuditRoute,
    pub(crate) method: HttpMethod,
    pub(crate) targets: Bounded<Vec<AuditTarget>, 64>,
    pub(crate) status: u16,
    pub(crate) api_code: Option<ErrorToken>,
    pub(crate) cause: Option<ErrorToken>,
}

impl sealed::Sealed for OperatorRequest {}

impl JournalEvent for OperatorRequest {
    const KIND: &'static str = "operator_request";
    const CATEGORY: Category = Category::Audit;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("route", &self.route);
        w.field("method", &self.method);
        w.field("targets", &self.targets);
        w.field("status", &self.status);
        w.field("api_code", &self.api_code);
        w.field("cause", &self.cause);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthSucceeded {}

impl sealed::Sealed for AuthSucceeded {}

impl JournalEvent for AuthSucceeded {
    const KIND: &'static str = "auth_succeeded";
    const CATEGORY: Category = Category::Auth;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, _w: &mut DetailsWriter) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthFailed {
    pub(crate) class: AuthFailureClass,
}

impl sealed::Sealed for AuthFailed {}

impl JournalEvent for AuthFailed {
    const KIND: &'static str = "auth_failed";
    const CATEGORY: Category = Category::Auth;
    const SEVERITY: Severity = Severity::Warning;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("class", &self.class);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LoggedOut {}

impl sealed::Sealed for LoggedOut {}

impl JournalEvent for LoggedOut {
    const KIND: &'static str = "logged_out";
    const CATEGORY: Category = Category::Auth;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, _w: &mut DetailsWriter) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PasswordChanged {}

impl sealed::Sealed for PasswordChanged {}

impl JournalEvent for PasswordChanged {
    const KIND: &'static str = "password_changed";
    const CATEGORY: Category = Category::Auth;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, _w: &mut DetailsWriter) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PasswordResetIssued {}

impl sealed::Sealed for PasswordResetIssued {}

impl JournalEvent for PasswordResetIssued {
    const KIND: &'static str = "password_reset_issued";
    const CATEGORY: Category = Category::Auth;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, _w: &mut DetailsWriter) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PasswordResetCompleted {}

impl sealed::Sealed for PasswordResetCompleted {}

impl JournalEvent for PasswordResetCompleted {
    const KIND: &'static str = "password_reset_completed";
    const CATEGORY: Category = Category::Auth;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, _w: &mut DetailsWriter) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InvitationRegistered {}

impl sealed::Sealed for InvitationRegistered {}

impl JournalEvent for InvitationRegistered {
    const KIND: &'static str = "invitation_registered";
    const CATEGORY: Category = Category::Auth;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, _w: &mut DetailsWriter) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServerStarted {
    pub(crate) boot_id: Uuid,
}

impl sealed::Sealed for ServerStarted {}

impl JournalEvent for ServerStarted {
    const KIND: &'static str = "server_started";
    const CATEGORY: Category = Category::Journal;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("boot_id", &self.boot_id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServerStopping {
    pub(crate) lost_on_flush: Option<u64>,
}

impl sealed::Sealed for ServerStopping {}

impl JournalEvent for ServerStopping {
    const KIND: &'static str = "server_stopping";
    const CATEGORY: Category = Category::Journal;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("lost_on_flush", &self.lost_on_flush);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JournalDropped {
    pub(crate) count: u64,
    pub(crate) since: i64,
    pub(crate) category: Category,
}

impl sealed::Sealed for JournalDropped {}

impl JournalEvent for JournalDropped {
    const KIND: &'static str = "journal_dropped";
    const CATEGORY: Category = Category::Journal;
    const SEVERITY: Severity = Severity::Warning;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("count", &self.count);
        w.field("since", &self.since);
        w.field("category", &self.category);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JournalPaused {
    pub(crate) reason: PauseReason,
}

impl sealed::Sealed for JournalPaused {}

impl JournalEvent for JournalPaused {
    const KIND: &'static str = "journal_paused";
    const CATEGORY: Category = Category::Journal;
    const SEVERITY: Severity = Severity::Warning;

    fn write(&self, w: &mut DetailsWriter) {
        w.field("reason", &self.reason);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JournalResumed {}

impl sealed::Sealed for JournalResumed {}

impl JournalEvent for JournalResumed {
    const KIND: &'static str = "journal_resumed";
    const CATEGORY: Category = Category::Journal;
    const SEVERITY: Severity = Severity::Info;

    fn write(&self, _w: &mut DetailsWriter) {}
}
