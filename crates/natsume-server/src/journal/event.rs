use serde::Deserialize;

mod contract;
pub(crate) mod payload;
pub(crate) mod types;

#[cfg(test)]
pub(crate) use contract::BoundError;
pub(crate) use contract::Bounded;
pub(crate) use contract::EventError;
use contract::{DetailsWriter, JournalEvent};
use payload::{
    ActivationCommitted, ActorStopped, ActualChanged, AdmissionCompleted, AdmissionRejected,
    AuthFailed, AuthSucceeded, BindingBound, BindingRejected, BindingUnbound, Disconnected,
    EnrollmentApproved, EnrollmentDenied, InvitationRegistered, JournalDropped, JournalPaused,
    JournalResumed, LeaseAttached, LeaseReplaced, LifecycleChanged, LoggedOut, OperatorRequest,
    PasswordChanged, PasswordResetCompleted, PasswordResetIssued, ReconcileRetry, ReviewCancelled,
    ReviewCreated, ServerStarted, ServerStopping, TargetSubmitted,
};
use types::{Category, Severity};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Event {
    AdmissionRejected(AdmissionRejected),
    LeaseAttached(LeaseAttached),
    LeaseReplaced(LeaseReplaced),
    Disconnected(Disconnected),
    ReconcileRetry(ReconcileRetry),
    ActorStopped(ActorStopped),
    ActualChanged(ActualChanged),
    ReviewCreated(ReviewCreated),
    ReviewCancelled(ReviewCancelled),
    EnrollmentApproved(EnrollmentApproved),
    EnrollmentDenied(EnrollmentDenied),
    ActivationCommitted(ActivationCommitted),
    AdmissionCompleted(AdmissionCompleted),
    LifecycleChanged(LifecycleChanged),
    BindingBound(BindingBound),
    BindingRejected(BindingRejected),
    BindingUnbound(BindingUnbound),
    TargetSubmitted(TargetSubmitted),
    OperatorRequest(OperatorRequest),
    AuthSucceeded(AuthSucceeded),
    AuthFailed(AuthFailed),
    LoggedOut(LoggedOut),
    PasswordChanged(PasswordChanged),
    PasswordResetIssued(PasswordResetIssued),
    PasswordResetCompleted(PasswordResetCompleted),
    InvitationRegistered(InvitationRegistered),
    ServerStarted(ServerStarted),
    ServerStopping(ServerStopping),
    JournalDropped(JournalDropped),
    JournalPaused(JournalPaused),
    JournalResumed(JournalResumed),
}

impl Event {
    pub(crate) const KINDS: &'static [&'static str] = &[
        AdmissionRejected::KIND,
        LeaseAttached::KIND,
        LeaseReplaced::KIND,
        Disconnected::KIND,
        ReconcileRetry::KIND,
        ActorStopped::KIND,
        ActualChanged::KIND,
        ReviewCreated::KIND,
        ReviewCancelled::KIND,
        EnrollmentApproved::KIND,
        EnrollmentDenied::KIND,
        ActivationCommitted::KIND,
        AdmissionCompleted::KIND,
        LifecycleChanged::KIND,
        BindingBound::KIND,
        BindingRejected::KIND,
        BindingUnbound::KIND,
        TargetSubmitted::KIND,
        OperatorRequest::KIND,
        AuthSucceeded::KIND,
        AuthFailed::KIND,
        LoggedOut::KIND,
        PasswordChanged::KIND,
        PasswordResetIssued::KIND,
        PasswordResetCompleted::KIND,
        InvitationRegistered::KIND,
        ServerStarted::KIND,
        ServerStopping::KIND,
        JournalDropped::KIND,
        JournalPaused::KIND,
        JournalResumed::KIND,
    ];

    pub(crate) fn category_for_kind(kind: &str) -> Option<Category> {
        match kind {
            AdmissionRejected::KIND => Some(AdmissionRejected::CATEGORY),
            LeaseAttached::KIND => Some(LeaseAttached::CATEGORY),
            LeaseReplaced::KIND => Some(LeaseReplaced::CATEGORY),
            Disconnected::KIND => Some(Disconnected::CATEGORY),
            ReconcileRetry::KIND => Some(ReconcileRetry::CATEGORY),
            ActorStopped::KIND => Some(ActorStopped::CATEGORY),
            ActualChanged::KIND => Some(ActualChanged::CATEGORY),
            ReviewCreated::KIND => Some(ReviewCreated::CATEGORY),
            ReviewCancelled::KIND => Some(ReviewCancelled::CATEGORY),
            EnrollmentApproved::KIND => Some(EnrollmentApproved::CATEGORY),
            EnrollmentDenied::KIND => Some(EnrollmentDenied::CATEGORY),
            ActivationCommitted::KIND => Some(ActivationCommitted::CATEGORY),
            AdmissionCompleted::KIND => Some(AdmissionCompleted::CATEGORY),
            LifecycleChanged::KIND => Some(LifecycleChanged::CATEGORY),
            BindingBound::KIND => Some(BindingBound::CATEGORY),
            BindingRejected::KIND => Some(BindingRejected::CATEGORY),
            BindingUnbound::KIND => Some(BindingUnbound::CATEGORY),
            TargetSubmitted::KIND => Some(TargetSubmitted::CATEGORY),
            OperatorRequest::KIND => Some(OperatorRequest::CATEGORY),
            AuthSucceeded::KIND => Some(AuthSucceeded::CATEGORY),
            AuthFailed::KIND => Some(AuthFailed::CATEGORY),
            LoggedOut::KIND => Some(LoggedOut::CATEGORY),
            PasswordChanged::KIND => Some(PasswordChanged::CATEGORY),
            PasswordResetIssued::KIND => Some(PasswordResetIssued::CATEGORY),
            PasswordResetCompleted::KIND => Some(PasswordResetCompleted::CATEGORY),
            InvitationRegistered::KIND => Some(InvitationRegistered::CATEGORY),
            ServerStarted::KIND => Some(ServerStarted::CATEGORY),
            ServerStopping::KIND => Some(ServerStopping::CATEGORY),
            JournalDropped::KIND => Some(JournalDropped::CATEGORY),
            JournalPaused::KIND => Some(JournalPaused::CATEGORY),
            JournalResumed::KIND => Some(JournalResumed::CATEGORY),
            _ => None,
        }
    }

    pub(crate) const fn kind(&self) -> &'static str {
        match self {
            Self::AdmissionRejected(..) => AdmissionRejected::KIND,
            Self::LeaseAttached(..) => LeaseAttached::KIND,
            Self::LeaseReplaced(..) => LeaseReplaced::KIND,
            Self::Disconnected(..) => Disconnected::KIND,
            Self::ReconcileRetry(..) => ReconcileRetry::KIND,
            Self::ActorStopped(..) => ActorStopped::KIND,
            Self::ActualChanged(..) => ActualChanged::KIND,
            Self::ReviewCreated(..) => ReviewCreated::KIND,
            Self::ReviewCancelled(..) => ReviewCancelled::KIND,
            Self::EnrollmentApproved(..) => EnrollmentApproved::KIND,
            Self::EnrollmentDenied(..) => EnrollmentDenied::KIND,
            Self::ActivationCommitted(..) => ActivationCommitted::KIND,
            Self::AdmissionCompleted(..) => AdmissionCompleted::KIND,
            Self::LifecycleChanged(..) => LifecycleChanged::KIND,
            Self::BindingBound(..) => BindingBound::KIND,
            Self::BindingRejected(..) => BindingRejected::KIND,
            Self::BindingUnbound(..) => BindingUnbound::KIND,
            Self::TargetSubmitted(..) => TargetSubmitted::KIND,
            Self::OperatorRequest(..) => OperatorRequest::KIND,
            Self::AuthSucceeded(..) => AuthSucceeded::KIND,
            Self::AuthFailed(..) => AuthFailed::KIND,
            Self::LoggedOut(..) => LoggedOut::KIND,
            Self::PasswordChanged(..) => PasswordChanged::KIND,
            Self::PasswordResetIssued(..) => PasswordResetIssued::KIND,
            Self::PasswordResetCompleted(..) => PasswordResetCompleted::KIND,
            Self::InvitationRegistered(..) => InvitationRegistered::KIND,
            Self::ServerStarted(..) => ServerStarted::KIND,
            Self::ServerStopping(..) => ServerStopping::KIND,
            Self::JournalDropped(..) => JournalDropped::KIND,
            Self::JournalPaused(..) => JournalPaused::KIND,
            Self::JournalResumed(..) => JournalResumed::KIND,
        }
    }

    pub(crate) const fn category(&self) -> Category {
        match self {
            Self::AdmissionRejected(..) => AdmissionRejected::CATEGORY,
            Self::LeaseAttached(..) => LeaseAttached::CATEGORY,
            Self::LeaseReplaced(..) => LeaseReplaced::CATEGORY,
            Self::Disconnected(..) => Disconnected::CATEGORY,
            Self::ReconcileRetry(..) => ReconcileRetry::CATEGORY,
            Self::ActorStopped(..) => ActorStopped::CATEGORY,
            Self::ActualChanged(..) => ActualChanged::CATEGORY,
            Self::ReviewCreated(..) => ReviewCreated::CATEGORY,
            Self::ReviewCancelled(..) => ReviewCancelled::CATEGORY,
            Self::EnrollmentApproved(..) => EnrollmentApproved::CATEGORY,
            Self::EnrollmentDenied(..) => EnrollmentDenied::CATEGORY,
            Self::ActivationCommitted(..) => ActivationCommitted::CATEGORY,
            Self::AdmissionCompleted(..) => AdmissionCompleted::CATEGORY,
            Self::LifecycleChanged(..) => LifecycleChanged::CATEGORY,
            Self::BindingBound(..) => BindingBound::CATEGORY,
            Self::BindingRejected(..) => BindingRejected::CATEGORY,
            Self::BindingUnbound(..) => BindingUnbound::CATEGORY,
            Self::TargetSubmitted(..) => TargetSubmitted::CATEGORY,
            Self::OperatorRequest(..) => OperatorRequest::CATEGORY,
            Self::AuthSucceeded(..) => AuthSucceeded::CATEGORY,
            Self::AuthFailed(..) => AuthFailed::CATEGORY,
            Self::LoggedOut(..) => LoggedOut::CATEGORY,
            Self::PasswordChanged(..) => PasswordChanged::CATEGORY,
            Self::PasswordResetIssued(..) => PasswordResetIssued::CATEGORY,
            Self::PasswordResetCompleted(..) => PasswordResetCompleted::CATEGORY,
            Self::InvitationRegistered(..) => InvitationRegistered::CATEGORY,
            Self::ServerStarted(..) => ServerStarted::CATEGORY,
            Self::ServerStopping(..) => ServerStopping::CATEGORY,
            Self::JournalDropped(..) => JournalDropped::CATEGORY,
            Self::JournalPaused(..) => JournalPaused::CATEGORY,
            Self::JournalResumed(..) => JournalResumed::CATEGORY,
        }
    }

    pub(crate) const fn severity(&self) -> Severity {
        match self {
            Self::AdmissionRejected(..) => AdmissionRejected::SEVERITY,
            Self::LeaseAttached(..) => LeaseAttached::SEVERITY,
            Self::LeaseReplaced(..) => LeaseReplaced::SEVERITY,
            Self::Disconnected(..) => Disconnected::SEVERITY,
            Self::ReconcileRetry(..) => ReconcileRetry::SEVERITY,
            Self::ActorStopped(..) => ActorStopped::SEVERITY,
            Self::ActualChanged(..) => ActualChanged::SEVERITY,
            Self::ReviewCreated(..) => ReviewCreated::SEVERITY,
            Self::ReviewCancelled(..) => ReviewCancelled::SEVERITY,
            Self::EnrollmentApproved(..) => EnrollmentApproved::SEVERITY,
            Self::EnrollmentDenied(..) => EnrollmentDenied::SEVERITY,
            Self::ActivationCommitted(..) => ActivationCommitted::SEVERITY,
            Self::AdmissionCompleted(..) => AdmissionCompleted::SEVERITY,
            Self::LifecycleChanged(..) => LifecycleChanged::SEVERITY,
            Self::BindingBound(..) => BindingBound::SEVERITY,
            Self::BindingRejected(..) => BindingRejected::SEVERITY,
            Self::BindingUnbound(..) => BindingUnbound::SEVERITY,
            Self::TargetSubmitted(..) => TargetSubmitted::SEVERITY,
            Self::OperatorRequest(..) => OperatorRequest::SEVERITY,
            Self::AuthSucceeded(..) => AuthSucceeded::SEVERITY,
            Self::AuthFailed(..) => AuthFailed::SEVERITY,
            Self::LoggedOut(..) => LoggedOut::SEVERITY,
            Self::PasswordChanged(..) => PasswordChanged::SEVERITY,
            Self::PasswordResetIssued(..) => PasswordResetIssued::SEVERITY,
            Self::PasswordResetCompleted(..) => PasswordResetCompleted::SEVERITY,
            Self::InvitationRegistered(..) => InvitationRegistered::SEVERITY,
            Self::ServerStarted(..) => ServerStarted::SEVERITY,
            Self::ServerStopping(..) => ServerStopping::SEVERITY,
            Self::JournalDropped(..) => JournalDropped::SEVERITY,
            Self::JournalPaused(..) => JournalPaused::SEVERITY,
            Self::JournalResumed(..) => JournalResumed::SEVERITY,
        }
    }

    pub(super) fn details(&self) -> Result<String, EventError> {
        match self {
            Self::AdmissionRejected(payload) => DetailsWriter::encode(payload),
            Self::LeaseAttached(payload) => DetailsWriter::encode(payload),
            Self::LeaseReplaced(payload) => DetailsWriter::encode(payload),
            Self::Disconnected(payload) => DetailsWriter::encode(payload),
            Self::ReconcileRetry(payload) => DetailsWriter::encode(payload),
            Self::ActorStopped(payload) => DetailsWriter::encode(payload),
            Self::ActualChanged(payload) => DetailsWriter::encode(payload),
            Self::ReviewCreated(payload) => DetailsWriter::encode(payload),
            Self::ReviewCancelled(payload) => DetailsWriter::encode(payload),
            Self::EnrollmentApproved(payload) => DetailsWriter::encode(payload),
            Self::EnrollmentDenied(payload) => DetailsWriter::encode(payload),
            Self::ActivationCommitted(payload) => DetailsWriter::encode(payload),
            Self::AdmissionCompleted(payload) => DetailsWriter::encode(payload),
            Self::LifecycleChanged(payload) => DetailsWriter::encode(payload),
            Self::BindingBound(payload) => DetailsWriter::encode(payload),
            Self::BindingRejected(payload) => DetailsWriter::encode(payload),
            Self::BindingUnbound(payload) => DetailsWriter::encode(payload),
            Self::TargetSubmitted(payload) => DetailsWriter::encode(payload),
            Self::OperatorRequest(payload) => DetailsWriter::encode(payload),
            Self::AuthSucceeded(payload) => DetailsWriter::encode(payload),
            Self::AuthFailed(payload) => DetailsWriter::encode(payload),
            Self::LoggedOut(payload) => DetailsWriter::encode(payload),
            Self::PasswordChanged(payload) => DetailsWriter::encode(payload),
            Self::PasswordResetIssued(payload) => DetailsWriter::encode(payload),
            Self::PasswordResetCompleted(payload) => DetailsWriter::encode(payload),
            Self::InvitationRegistered(payload) => DetailsWriter::encode(payload),
            Self::ServerStarted(payload) => DetailsWriter::encode(payload),
            Self::ServerStopping(payload) => DetailsWriter::encode(payload),
            Self::JournalDropped(payload) => DetailsWriter::encode(payload),
            Self::JournalPaused(payload) => DetailsWriter::encode(payload),
            Self::JournalResumed(payload) => DetailsWriter::encode(payload),
        }
    }

    fn decode(kind: &str, details: &str) -> Result<Self, serde_json::Error> {
        match kind {
            AdmissionRejected::KIND => decode_payload(details).map(Self::AdmissionRejected),
            LeaseAttached::KIND => decode_payload(details).map(Self::LeaseAttached),
            LeaseReplaced::KIND => decode_payload(details).map(Self::LeaseReplaced),
            Disconnected::KIND => decode_payload(details).map(Self::Disconnected),
            ReconcileRetry::KIND => decode_payload(details).map(Self::ReconcileRetry),
            ActorStopped::KIND => decode_payload(details).map(Self::ActorStopped),
            ActualChanged::KIND => decode_payload(details).map(Self::ActualChanged),
            ReviewCreated::KIND => decode_payload(details).map(Self::ReviewCreated),
            ReviewCancelled::KIND => decode_payload(details).map(Self::ReviewCancelled),
            EnrollmentApproved::KIND => decode_payload(details).map(Self::EnrollmentApproved),
            EnrollmentDenied::KIND => decode_payload(details).map(Self::EnrollmentDenied),
            ActivationCommitted::KIND => decode_payload(details).map(Self::ActivationCommitted),
            AdmissionCompleted::KIND => decode_payload(details).map(Self::AdmissionCompleted),
            LifecycleChanged::KIND => decode_payload(details).map(Self::LifecycleChanged),
            BindingBound::KIND => decode_payload(details).map(Self::BindingBound),
            BindingRejected::KIND => decode_payload(details).map(Self::BindingRejected),
            BindingUnbound::KIND => decode_payload(details).map(Self::BindingUnbound),
            TargetSubmitted::KIND => decode_payload(details).map(Self::TargetSubmitted),
            OperatorRequest::KIND => decode_payload(details).map(Self::OperatorRequest),
            AuthSucceeded::KIND => decode_payload(details).map(Self::AuthSucceeded),
            AuthFailed::KIND => decode_payload(details).map(Self::AuthFailed),
            LoggedOut::KIND => decode_payload(details).map(Self::LoggedOut),
            PasswordChanged::KIND => decode_payload(details).map(Self::PasswordChanged),
            PasswordResetIssued::KIND => decode_payload(details).map(Self::PasswordResetIssued),
            PasswordResetCompleted::KIND => {
                decode_payload(details).map(Self::PasswordResetCompleted)
            }
            InvitationRegistered::KIND => decode_payload(details).map(Self::InvitationRegistered),
            ServerStarted::KIND => decode_payload(details).map(Self::ServerStarted),
            ServerStopping::KIND => decode_payload(details).map(Self::ServerStopping),
            JournalDropped::KIND => decode_payload(details).map(Self::JournalDropped),
            JournalPaused::KIND => decode_payload(details).map(Self::JournalPaused),
            JournalResumed::KIND => decode_payload(details).map(Self::JournalResumed),
            _ => Err(serde::de::Error::custom("unknown journal kind")),
        }
    }
}

impl serde::Serialize for Event {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;
        let encoded = self.details().map_err(S::Error::custom)?;
        let mut fields: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&encoded).map_err(S::Error::custom)?;
        fields.remove("v");
        fields.serialize(serializer)
    }
}

fn decode_payload<T: JournalEvent + serde::de::DeserializeOwned>(
    details: &str,
) -> Result<T, serde_json::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Versioned<T> {
        kind: String,
        v: u8,
        #[serde(flatten)]
        payload: T,
    }
    let wire: Versioned<T> = serde_json::from_str(details)?;
    if wire.v != 1 || wire.kind != T::KIND {
        return Err(serde::de::Error::custom(
            "unsupported journal kind or version",
        ));
    }
    Ok(wire.payload)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReadEvent {
    Known(Event),
    Unreadable { id: i64, kind: String },
}

impl ReadEvent {
    pub(super) fn from_persisted(id: i64, kind: &str, details: &str) -> Self {
        if details.len() <= 2048 {
            match Event::decode(kind, details) {
                Ok(event) => return Self::Known(event),
                Err(error) => tracing::warn!(
                    id,
                    error_type = "serde_json",
                    line = error.line(),
                    column = error.column(),
                    "unreadable journal event"
                ),
            }
        }
        let kind = if !kind.is_empty()
            && kind.len() <= 64
            && kind
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            kind.to_owned()
        } else {
            "unknown".to_owned()
        };
        Self::Unreadable { id, kind }
    }
}

#[cfg(test)]
mod tests {
    use super::types::SessionId;
    use super::*;
    use crate::component::device::{DeviceId, EnrollmentReviewId};
    use contract::{BoundError, Bounded};
    use snafu::Snafu;
    use std::net::IpAddr;
    use types::{
        ActualResource, ActualState, AdmissionStage, AuditRoute, AuditTarget, AuthFailureClass,
        BindingCode, DisconnectReason, ErrorToken, HttpMethod, LifecycleState, PauseReason,
        ReviewCancelCause, SeatCode, TargetAction, TargetResult,
    };
    use types::{OperatorId, ReviewId};
    use uuid::Uuid;

    #[derive(Debug, Snafu)]
    enum TestError {
        #[snafu(context(false), display("event test JSON failed"))]
        Json { source: serde_json::Error },
        #[snafu(context(false), display("event test encoding failed"))]
        Event { source: EventError },
        #[snafu(context(false), display("event test bounded value failed"))]
        Bound { source: BoundError },
        #[snafu(display("event test device fixture is invalid"))]
        InvalidFixture,
    }

    #[test]
    fn session_newtypes_preserve_the_existing_json_uuid_shape() -> Result<(), TestError> {
        let id = Uuid::now_v7();
        let event = Event::LeaseAttached(LeaseAttached {
            session_id: SessionId::from_lease(id),
        });
        let details: serde_json::Value = serde_json::from_str(&event.details()?)?;
        assert_eq!(details["session_id"], id.to_string());
        assert_eq!(Event::decode(event.kind(), &event.details()?)?, event);
        Ok(())
    }

    #[test]
    fn review_ids_are_whitelisted_uuid_strings_and_reject_invalid_values() -> Result<(), TestError>
    {
        let raw = "019a1000-0000-7000-8000-000000000001";
        let review = crate::component::device::EnrollmentReviewId::parse(raw)
            .ok_or(TestError::InvalidFixture)?;
        let review_id = types::ReviewId::from_enrollment_review(review);
        let events = [
            Event::ReviewCreated(ReviewCreated {
                review_id,
                peer_ip: IpAddr::from([127, 0, 0, 1]),
            }),
            Event::ReviewCancelled(ReviewCancelled {
                review_id,
                cause: ReviewCancelCause::ClientClose,
            }),
            Event::EnrollmentApproved(EnrollmentApproved {
                review_id,
                automatic: false,
            }),
            Event::EnrollmentDenied(EnrollmentDenied { review_id }),
        ];
        for event in events {
            let encoded = event.details()?;
            let details: serde_json::Value = serde_json::from_str(&encoded)?;
            assert_eq!(details["review_id"], raw);
            assert_eq!(Event::decode(event.kind(), &encoded)?, event);
            assert!(Event::decode(event.kind(), &encoded.replace(raw, "invalid-review")).is_err());
        }
        assert!(serde_json::from_str::<types::ReviewId>("42").is_err());
        Ok(())
    }

    #[test]
    fn actual_baseline_and_unknown_transition_have_distinct_wire_shapes() -> Result<(), TestError> {
        for from in [None, Some(ActualState::Unknown)] {
            let event = Event::ActualChanged(ActualChanged {
                resource: ActualResource::Foreground,
                from: from.clone(),
                to: ActualState::Unknown,
            });
            let encoded = event.details()?;
            let details: serde_json::Value = serde_json::from_str(&encoded)?;
            assert_eq!(details["from"], serde_json::to_value(from)?);
            assert_eq!(Event::decode(event.kind(), &encoded)?, event);
        }
        Ok(())
    }

    fn samples() -> Result<Vec<Event>, TestError> {
        let id = Uuid::nil();
        let device_id = DeviceId::parse("019a1000-0000-7000-8000-000000000001")
            .ok_or(TestError::InvalidFixture)?;
        let review_id = ReviewId::from_enrollment_review(
            EnrollmentReviewId::parse("019a1000-0000-7000-8000-000000000001")
                .ok_or(TestError::InvalidFixture)?,
        );
        let seat = SeatCode::from_committed_binding("A-01")?;
        Ok(vec![
            Event::AdmissionRejected(AdmissionRejected {
                stage: AdmissionStage::Proof,
                peer_ip: IpAddr::from([127, 0, 0, 1]),
                device_id: Some(device_id),
            }),
            Event::LeaseAttached(LeaseAttached {
                session_id: SessionId::from_lease(id),
            }),
            Event::LeaseReplaced(LeaseReplaced {
                old: SessionId::from_lease(id),
                new: SessionId::from_lease(Uuid::max()),
            }),
            Event::Disconnected(Disconnected {
                session_id: SessionId::from_lease(id),
                reason: DisconnectReason::ClientClose,
            }),
            Event::ReconcileRetry(ReconcileRetry { attempt: u32::MAX }),
            Event::ActorStopped(ActorStopped {
                panicked: true,
                session_id: Some(SessionId::from_lease(id)),
            }),
            Event::ActualChanged(ActualChanged {
                resource: ActualResource::Session,
                from: Some(ActualState::Starting),
                to: ActualState::Running,
            }),
            Event::ReviewCreated(ReviewCreated {
                review_id,
                peer_ip: std::net::IpAddr::from([127, 0, 0, 1]),
            }),
            Event::ReviewCancelled(ReviewCancelled {
                review_id,
                cause: ReviewCancelCause::Timeout,
            }),
            Event::EnrollmentApproved(EnrollmentApproved {
                review_id,
                automatic: true,
            }),
            Event::EnrollmentDenied(EnrollmentDenied { review_id }),
            Event::ActivationCommitted(ActivationCommitted {}),
            Event::AdmissionCompleted(AdmissionCompleted {}),
            Event::LifecycleChanged(LifecycleChanged {
                to: LifecycleState::Disabled,
            }),
            Event::BindingBound(BindingBound {
                binding_id: id,
                seat_code: seat.clone(),
            }),
            Event::BindingRejected(BindingRejected {
                code: BindingCode::Occupied,
            }),
            Event::BindingUnbound(BindingUnbound { seat_code: seat }),
            Event::TargetSubmitted(TargetSubmitted {
                operation_id: id,
                action: TargetAction::ResetHome,
                result: TargetResult::Accepted,
                replayed: true,
            }),
            Event::OperatorRequest(OperatorRequest {
                route: AuditRoute::Session,
                method: HttpMethod::Post,
                targets: Bounded::from_typed_targets(vec![AuditTarget::Operator(
                    OperatorId::from_authenticated_operator(id),
                )])?,
                status: 403,
                api_code: Some(ErrorToken::from_static_api_code("FORBIDDEN")?),
                cause: None,
            }),
            Event::AuthSucceeded(AuthSucceeded {}),
            Event::AuthFailed(AuthFailed {
                class: AuthFailureClass::InvalidCredentials,
            }),
            Event::LoggedOut(LoggedOut {}),
            Event::PasswordChanged(PasswordChanged {}),
            Event::PasswordResetIssued(PasswordResetIssued {}),
            Event::PasswordResetCompleted(PasswordResetCompleted {}),
            Event::InvitationRegistered(InvitationRegistered {}),
            Event::ServerStarted(ServerStarted { boot_id: id }),
            Event::ServerStopping(ServerStopping {
                lost_on_flush: Some(u64::MAX),
            }),
            Event::JournalDropped(JournalDropped {
                count: u64::MAX,
                since: i64::MIN,
                category: Category::Device,
            }),
            Event::JournalPaused(JournalPaused {
                reason: PauseReason::DiskBudget,
            }),
            Event::JournalResumed(JournalResumed {}),
        ])
    }

    #[test]
    fn every_variant_round_trips_with_version_and_catalog_metadata() -> Result<(), TestError> {
        let events = samples()?;
        assert_eq!(events.len(), Event::KINDS.len());
        for (event, expected_kind) in events.iter().zip(Event::KINDS) {
            assert_eq!(event.kind(), *expected_kind);
            let encoded = event.details()?;
            let json: serde_json::Value = serde_json::from_str(&encoded)?;
            assert_eq!(json["v"], 1);
            assert_eq!(json["kind"], event.kind());
            assert_eq!(Event::decode(event.kind(), &encoded)?, *event);
            assert_eq!(
                ReadEvent::from_persisted(1, event.kind(), &encoded),
                ReadEvent::Known(event.clone())
            );
            assert!(encoded.len() <= 2048);
            let expected_category = match event {
                Event::OperatorRequest(..) => "audit",
                Event::AuthSucceeded(AuthSucceeded {})
                | Event::AuthFailed(..)
                | Event::LoggedOut(LoggedOut {})
                | Event::PasswordChanged(PasswordChanged {})
                | Event::PasswordResetIssued(PasswordResetIssued {})
                | Event::PasswordResetCompleted(PasswordResetCompleted {})
                | Event::InvitationRegistered(InvitationRegistered {}) => "auth",
                Event::ServerStarted(..)
                | Event::ServerStopping(..)
                | Event::JournalDropped(..)
                | Event::JournalPaused(..)
                | Event::JournalResumed(JournalResumed {}) => "journal",
                _ => "device",
            };
            assert_eq!(event.category().as_str(), expected_category);
            assert!(matches!(
                event.severity().as_str(),
                "info" | "warning" | "error"
            ));
        }
        assert_eq!(
            Event::ActorStopped(ActorStopped {
                panicked: true,
                session_id: None
            })
            .severity(),
            Severity::Error
        );
        assert_eq!(
            Event::AuthFailed(AuthFailed {
                class: AuthFailureClass::InvalidCredentials
            })
            .severity(),
            Severity::Warning
        );
        Ok(())
    }

    #[test]
    fn unknown_corrupt_mismatched_and_future_events_are_bounded_unreadable() {
        for (kind, details) in [
            ("future_event", "{\"kind\":\"future_event\",\"v\":1}"),
            (
                "server_started",
                "{\"kind\":\"server_started\",\"v\":2,\"boot_id\":\"00000000-0000-0000-0000-000000000000\"}",
            ),
            ("review_created", "{\"kind\":\"review_created\"}"),
            (
                "review_created",
                "{\"kind\":\"review_created\",\"v\":1,\"password\":\"CANARY\"}",
            ),
            ("review_created", "{\"kind\":\"journal_resumed\",\"v\":1}"),
            ("review_created", "invalid CANARY"),
            (
                "server_started",
                "{\"kind\":\"server_started\",\"v\":1,\"boot_id\":\"00000000-0000-0000-0000-000000000000\",\"password\":\"CANARY\"}",
            ),
            (
                "review_created",
                "{\"kind\":\"review_created\",\"v\":1,\"v\":1}",
            ),
            (
                "lease_attached",
                "{\"kind\":\"lease_attached\",\"v\":1,\"session_id\":\"00000000-0000-0000-0000-000000000000\",\"session_id\":\"00000000-0000-0000-0000-000000000000\"}",
            ),
        ] {
            assert_eq!(
                ReadEvent::from_persisted(42, kind, details),
                ReadEvent::Unreadable {
                    id: 42,
                    kind: kind.into()
                }
            );
        }
        assert_eq!(
            ReadEvent::from_persisted(42, &"x".repeat(65), "CANARY"),
            ReadEvent::Unreadable {
                id: 42,
                kind: "unknown".into()
            }
        );
        assert_eq!(
            ReadEvent::from_persisted(42, "CANARY\n", "CANARY"),
            ReadEvent::Unreadable {
                id: 42,
                kind: "unknown".into()
            }
        );
        assert!(matches!(
            ReadEvent::from_persisted(1, "review_created", &" ".repeat(2049)),
            ReadEvent::Unreadable { .. }
        ));
    }

    #[test]
    fn wrappers_validate_on_construction_and_deserialization() -> Result<(), TestError> {
        for value in ["", "a", "A-B", "A B", "1CODE", "É", &"A".repeat(65)] {
            assert!(serde_json::from_value::<ErrorToken>(serde_json::json!(value)).is_err());
        }
        assert!(ErrorToken::from_static_api_code("SEAT_OCCUPIED").is_ok());
        assert!(ErrorToken::from_static_api_code("Invalid").is_err());
        assert!(serde_json::from_value::<ErrorToken>(serde_json::json!("A".repeat(64))).is_ok());
        for value in ["", "A\n01", &"A".repeat(65)] {
            assert!(SeatCode::from_committed_binding(value).is_err());
            assert!(serde_json::from_value::<SeatCode>(serde_json::json!(value)).is_err());
        }
        assert!(Bounded::<Vec<Uuid>, 1>::from_typed_targets(vec![Uuid::nil(); 2]).is_err());
        assert!(
            serde_json::from_value::<Bounded<Vec<Uuid>, 1>>(serde_json::json!([
                Uuid::nil(),
                Uuid::nil()
            ]))
            .is_err()
        );
        let bounded = Bounded::<Vec<Uuid>, 1>::from_typed_targets(vec![Uuid::nil()])?;
        assert_eq!(
            serde_json::from_str::<Bounded<Vec<Uuid>, 1>>(&serde_json::to_string(&bounded)?)?,
            bounded
        );
        Ok(())
    }

    #[test]
    fn api_causes_are_static_bounded_lowercase_discriminants() {
        assert!(types::AuditCause::from_static_api_cause("operator_authentication_failed").is_ok());
        for value in [
            "",
            "UPPER",
            "token.value",
            "has whitespace",
            "1invalid",
            &"x".repeat(65),
        ] {
            assert!(serde_json::from_value::<types::AuditCause>(serde_json::json!(value)).is_err());
        }
    }

    #[test]
    fn large_typed_event_is_rejected_at_the_details_boundary() -> Result<(), TestError> {
        let event = Event::OperatorRequest(OperatorRequest {
            route: AuditRoute::Session,
            method: HttpMethod::Post,
            targets: Bounded::from_typed_targets(vec![
                AuditTarget::Device(
                    DeviceId::parse("019a1000-0000-7000-8000-000000000001")
                        .ok_or(TestError::InvalidFixture)?
                );
                64
            ])?,
            status: 500,
            api_code: Some(ErrorToken::from_static_api_code("INTERNAL")?),
            cause: Some(types::AuditCause::from_static_api_cause(
                "internal_failure",
            )?),
        });
        assert!(matches!(event.details(), Err(EventError::TooLarge)));
        Ok(())
    }

    #[test]
    fn forbidden_fields_and_external_trait_implementation_do_not_compile() {
        let tests = trybuild::TestCases::new();
        tests.compile_fail("tests/journal_ui/*.rs");
        tests.pass("tests/journal_pass/*.rs");
    }
}
