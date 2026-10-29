use super::{
    BindingArtifactState, GatewayState, HomeState, ObservedActualState, RuntimeConfigState,
    SessionForeground, SessionState,
};
use crate::journal::{
    Journal, JournalContext,
    event::{
        Event,
        payload::ActualChanged,
        types::{ActualResource, ActualState},
    },
};
use natsume_workstation_protocol::generated::PowerState;

struct ActualSummary {
    /// Closed gateway state from the validated observation.
    gateway: ActualState,
    /// Combined artifact state; mixed applied/absent artifacts are pending.
    binding: ActualState,
    /// Closed runtime configuration state from the validated observation.
    runtime: ActualState,
    /// Closed contest session state from the validated observation.
    session: ActualState,
    /// Closed foreground state from the validated observation.
    foreground: ActualState,
    /// Closed home state from the validated observation.
    home: ActualState,
    /// Closed power state; unknown or absent reports use Unknown without changing validation.
    power: ActualState,
}

impl From<&ObservedActualState> for ActualSummary {
    fn from(actual: &ObservedActualState) -> Self {
        Self {
            gateway: match actual.gateway.state {
                GatewayState::Absent => ActualState::Absent,
                GatewayState::Blocked => ActualState::Blocked,
                GatewayState::Restoring => ActualState::Restoring,
                GatewayState::Ready => ActualState::Ready,
                GatewayState::UpstreamUnhealthy => ActualState::UpstreamUnhealthy,
                GatewayState::RecoveryRequired => ActualState::RecoveryRequired,
            },
            binding: match (
                actual.binding.assignment_state,
                actual.binding.credential_state,
            ) {
                (BindingArtifactState::Failed, _) | (_, BindingArtifactState::Failed) => {
                    ActualState::Failed
                }
                (BindingArtifactState::Applied, BindingArtifactState::Applied) => {
                    ActualState::Applied
                }
                (BindingArtifactState::Absent, BindingArtifactState::Absent) => ActualState::Absent,
                (BindingArtifactState::Applied, BindingArtifactState::Absent)
                | (BindingArtifactState::Absent, BindingArtifactState::Applied) => {
                    ActualState::Pending
                }
            },
            runtime: match actual.runtime_config.state {
                RuntimeConfigState::Absent => ActualState::Absent,
                RuntimeConfigState::Applied => ActualState::Applied,
                RuntimeConfigState::Failed => ActualState::Failed,
            },
            session: match actual.session_control.session_state {
                SessionState::None => ActualState::None,
                SessionState::Starting => ActualState::Starting,
                SessionState::Running => ActualState::Running,
                SessionState::Terminating => ActualState::Terminating,
                SessionState::Ambiguous => ActualState::Ambiguous,
                SessionState::Error => ActualState::Error,
            },
            foreground: match actual.session_control.foreground {
                SessionForeground::Unknown => ActualState::Unknown,
                SessionForeground::Waiting => ActualState::Waiting,
                SessionForeground::Contest => ActualState::Contest,
                SessionForeground::Greeter => ActualState::Greeter,
                SessionForeground::Other => ActualState::Other,
                SessionForeground::None => ActualState::None,
            },
            home: match actual.home.state {
                HomeState::Steady => ActualState::Steady,
                HomeState::Resetting => ActualState::Resetting,
                HomeState::RecoveryRequired => ActualState::RecoveryRequired,
            },
            power: match actual.power {
                None | Some(PowerState::Unspecified) => ActualState::Unknown,
                Some(PowerState::Idle) => ActualState::Idle,
                Some(PowerState::Pending) => ActualState::Pending,
                Some(PowerState::Accepted) => ActualState::Accepted,
                Some(PowerState::Expired) => ActualState::Expired,
                Some(PowerState::Error) => ActualState::Error,
            },
        }
    }
}

impl ObservedActualState {
    pub(in crate::device_control) fn emit_journal_changes(
        &self,
        previous: Option<&Self>,
        journal: &Journal,
        context: &JournalContext,
    ) {
        let next = ActualSummary::from(self);
        let previous = previous.map(ActualSummary::from);
        let emit = |resource, from: Option<&ActualState>, to: ActualState| {
            if from != Some(&to) {
                journal.emit(
                    Event::ActualChanged(ActualChanged {
                        resource,
                        from: from.cloned(),
                        to,
                    }),
                    context.clone(),
                );
            }
        };
        emit(
            ActualResource::Gateway,
            previous.as_ref().map(|state| &state.gateway),
            next.gateway,
        );
        emit(
            ActualResource::BindingAccess,
            previous.as_ref().map(|state| &state.binding),
            next.binding,
        );
        emit(
            ActualResource::RuntimeConfig,
            previous.as_ref().map(|state| &state.runtime),
            next.runtime,
        );
        emit(
            ActualResource::Session,
            previous.as_ref().map(|state| &state.session),
            next.session,
        );
        emit(
            ActualResource::Foreground,
            previous.as_ref().map(|state| &state.foreground),
            next.foreground,
        );
        emit(
            ActualResource::Home,
            previous.as_ref().map(|state| &state.home),
            next.home,
        );
        emit(
            ActualResource::Power,
            previous.as_ref().map(|state| &state.power),
            next.power,
        );
    }
}
