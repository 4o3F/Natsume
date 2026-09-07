use natsume_device_protocol::generated::{
    SessionControlActualState, SessionForeground as WireForeground,
    SessionState as WireSessionState,
};

use super::{
    ConvergenceStatus,
    binding::{BindingActual, BindingTarget, binding_convergence_status},
    home::{HomeActual, HomeState},
};
use crate::component::session::{ForegroundTarget, SessionControlTarget};

/// Session Control target, Actual, and convergence result.
#[derive(PartialEq, Eq)]
pub(crate) struct SessionConvergence {
    pub(crate) status: ConvergenceStatus,
    pub(crate) target: Option<SessionControlTarget>,
    pub(crate) actual: Option<SessionActual>,
}

/// Fresh display facts reported by the current control lease, never durable authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionActual {
    pub(crate) session_state: SessionState,
    pub(crate) completed_terminate_epoch: Option<u64>,
    pub(crate) foreground: SessionForeground,
    pub(crate) waiting_ready: bool,
    pub(crate) contest_ready: bool,
}

/// Contest lifecycle, independent of foreground and GNOME locking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionState {
    None,
    Starting,
    Running,
    Terminating,
    Ambiguous,
    Error,
}

/// Observed seat0 foreground, including unmanageable or unavailable displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionForeground {
    Unknown,
    Waiting,
    Contest,
    Greeter,
    Other,
    None,
}

pub(super) fn parse_session_actual(actual: SessionControlActualState) -> Option<SessionActual> {
    let session_state = match WireSessionState::try_from(actual.session_state).ok()? {
        WireSessionState::Unspecified => return None,
        WireSessionState::None => SessionState::None,
        WireSessionState::Starting => SessionState::Starting,
        WireSessionState::Running => SessionState::Running,
        WireSessionState::Terminating => SessionState::Terminating,
        WireSessionState::Ambiguous => SessionState::Ambiguous,
        WireSessionState::Error => SessionState::Error,
    };
    let foreground = match WireForeground::try_from(actual.foreground).ok()? {
        WireForeground::Unknown => SessionForeground::Unknown,
        WireForeground::Waiting => SessionForeground::Waiting,
        WireForeground::Contest => SessionForeground::Contest,
        WireForeground::Greeter => SessionForeground::Greeter,
        WireForeground::Other => SessionForeground::Other,
        WireForeground::None => SessionForeground::None,
    };
    if actual
        .completed_terminate_epoch
        .is_some_and(|epoch| epoch == 0 || epoch > i64::MAX.cast_unsigned())
        || (actual.contest_ready && session_state != SessionState::Running)
    {
        return None;
    }
    Some(SessionActual {
        session_state,
        completed_terminate_epoch: actual.completed_terminate_epoch,
        foreground,
        waiting_ready: actual.waiting_ready,
        contest_ready: actual.contest_ready,
    })
}

pub(super) fn session_convergence_status(
    target: Option<(ForegroundTarget, Option<u64>)>,
    actual: Option<&SessionActual>,
    home_target: Option<u64>,
    home: Option<&HomeActual>,
    binding_target: Option<&BindingTarget>,
    binding: Option<&BindingActual>,
) -> ConvergenceStatus {
    let (Some((foreground_target, terminate_epoch)), Some(actual)) = (target, actual) else {
        return ConvergenceStatus::AwaitingActual;
    };
    if terminate_epoch != actual.completed_terminate_epoch {
        return if terminate_epoch > actual.completed_terminate_epoch {
            ConvergenceStatus::Reconciling
        } else {
            ConvergenceStatus::Drifted
        };
    }
    let foreground_matches = match foreground_target {
        ForegroundTarget::Waiting => {
            actual.waiting_ready && actual.foreground == SessionForeground::Waiting
        }
        ForegroundTarget::Contest => {
            if matches!(
                actual.session_state,
                SessionState::Ambiguous | SessionState::Error
            ) {
                return ConvergenceStatus::Failed;
            }
            let Some(home) = home else {
                return ConvergenceStatus::AwaitingActual;
            };
            if home.state == HomeState::RecoveryRequired {
                return ConvergenceStatus::Failed;
            }
            if home.state != HomeState::Steady
                || home.completed_reset_epoch != home_target
                || actual.session_state != SessionState::Running
                || !actual.contest_ready
                || !matches!(binding_target, Some(BindingTarget::Bound { .. }))
                || binding_convergence_status(binding_target, binding)
                    != ConvergenceStatus::Converged
            {
                return ConvergenceStatus::Reconciling;
            }
            actual.foreground == SessionForeground::Contest
        }
    };
    if foreground_matches {
        ConvergenceStatus::Converged
    } else {
        ConvergenceStatus::Drifted
    }
}

#[cfg(test)]
mod tests {
    use super::super::binding::{BindingArtifactState, BindingContext};
    use super::*;

    fn fixture() -> (SessionActual, HomeActual, BindingTarget, BindingActual) {
        let context = BindingContext {
            binding_id: "01900000-0000-7000-8000-000000000001".to_owned(),
            account_id: "01900000-0000-7000-8000-000000000002".to_owned(),
            seat_code: "A-01".to_owned(),
            domjudge_username: "team1".to_owned(),
            credential_revision: 1,
        };
        (
            SessionActual {
                session_state: SessionState::Running,
                completed_terminate_epoch: Some(3),
                foreground: SessionForeground::Contest,
                waiting_ready: true,
                contest_ready: true,
            },
            HomeActual {
                state: HomeState::Steady,
                completed_reset_epoch: Some(4),
            },
            BindingTarget::Bound {
                context: context.clone(),
            },
            BindingActual {
                assignment_state: BindingArtifactState::Applied,
                credential_state: BindingArtifactState::Applied,
                context: Some(context),
            },
        )
    }

    #[test]
    fn contest_requires_running_ready_home_binding_and_actual_foreground() {
        let (mut actual, mut home, binding_target, mut binding) = fixture();
        let converged = |actual: &SessionActual, home: &HomeActual, binding: &BindingActual| {
            session_convergence_status(
                Some((ForegroundTarget::Contest, Some(3))),
                Some(actual),
                Some(4),
                Some(home),
                Some(&binding_target),
                Some(binding),
            ) == ConvergenceStatus::Converged
        };
        assert!(converged(&actual, &home, &binding));
        for lifecycle in [
            SessionState::None,
            SessionState::Starting,
            SessionState::Terminating,
            SessionState::Ambiguous,
            SessionState::Error,
        ] {
            actual.session_state = lifecycle;
            assert!(!converged(&actual, &home, &binding));
        }
        actual.session_state = SessionState::Running;
        actual.contest_ready = false;
        assert!(!converged(&actual, &home, &binding));
        actual.contest_ready = true;
        for foreground in [
            SessionForeground::Waiting,
            SessionForeground::Greeter,
            SessionForeground::Other,
            SessionForeground::None,
            SessionForeground::Unknown,
        ] {
            actual.foreground = foreground;
            assert!(!converged(&actual, &home, &binding));
        }
        actual.foreground = SessionForeground::Contest;
        for epoch in [None, Some(3), Some(5)] {
            home.completed_reset_epoch = epoch;
            assert!(!converged(&actual, &home, &binding));
        }
        home.completed_reset_epoch = Some(4);
        for state in [HomeState::Resetting, HomeState::RecoveryRequired] {
            home.state = state;
            assert!(!converged(&actual, &home, &binding));
        }
        home.state = HomeState::Steady;
        binding.credential_state = BindingArtifactState::Absent;
        assert!(!converged(&actual, &home, &binding));
        binding.credential_state = BindingArtifactState::Applied;
        binding
            .context
            .as_mut()
            .unwrap_or_else(|| panic!("bound context"))
            .credential_revision += 1;
        assert!(!converged(&actual, &home, &binding));
    }

    #[test]
    fn unbound_devices_wait_for_binding_without_reporting_contest_success() {
        let (actual, home, _, _) = fixture();
        let target = BindingTarget::Unbound {
            negotiation_id: "negotiation".to_owned(),
            evaluation: None,
        };
        let binding = BindingActual {
            assignment_state: BindingArtifactState::Absent,
            credential_state: BindingArtifactState::Absent,
            context: None,
        };
        assert_eq!(
            session_convergence_status(
                Some((ForegroundTarget::Contest, Some(3))),
                Some(&actual),
                Some(4),
                Some(&home),
                Some(&target),
                Some(&binding)
            ),
            ConvergenceStatus::Reconciling
        );
    }

    #[test]
    fn waiting_requires_its_own_display_without_a_binding_or_contest_dependency() {
        let (mut actual, _, _, _) = fixture();
        actual.session_state = SessionState::None;
        actual.contest_ready = false;
        actual.foreground = SessionForeground::Waiting;
        let status = |actual: &SessionActual| {
            session_convergence_status(
                Some((ForegroundTarget::Waiting, Some(3))),
                Some(actual),
                None,
                None,
                None,
                None,
            )
        };
        assert_eq!(status(&actual), ConvergenceStatus::Converged);
        actual.waiting_ready = false;
        assert_ne!(status(&actual), ConvergenceStatus::Converged);
        actual.waiting_ready = true;
        for foreground in [
            SessionForeground::Contest,
            SessionForeground::Greeter,
            SessionForeground::Other,
            SessionForeground::None,
            SessionForeground::Unknown,
        ] {
            actual.foreground = foreground;
            assert_ne!(status(&actual), ConvergenceStatus::Converged);
        }
        actual.foreground = SessionForeground::Waiting;
        for epoch in [None, Some(2), Some(4)] {
            actual.completed_terminate_epoch = epoch;
            assert_ne!(status(&actual), ConvergenceStatus::Converged);
        }
    }

    #[test]
    fn unknown_or_contradictory_display_facts_never_grant_convergence() {
        let baseline = SessionControlActualState {
            session_state: WireSessionState::Running.into(),
            ..SessionControlActualState::default()
        };
        let parsed = parse_session_actual(baseline).unwrap_or_else(|| panic!("known lifecycle"));
        assert_eq!(parsed.foreground, SessionForeground::Unknown);
        assert!(!parsed.waiting_ready && !parsed.contest_ready);
        assert_ne!(
            session_convergence_status(
                Some((ForegroundTarget::Waiting, None)),
                Some(&parsed),
                None,
                None,
                None,
                None
            ),
            ConvergenceStatus::Converged
        );
        for invalid in [
            SessionControlActualState {
                session_state: 99,
                ..baseline
            },
            SessionControlActualState {
                foreground: 99,
                ..baseline
            },
            SessionControlActualState {
                completed_terminate_epoch: Some(0),
                ..baseline
            },
            SessionControlActualState {
                completed_terminate_epoch: Some(u64::MAX),
                ..baseline
            },
            SessionControlActualState {
                session_state: WireSessionState::None.into(),
                contest_ready: true,
                ..baseline
            },
        ] {
            assert!(parse_session_actual(invalid).is_none());
        }
    }
}
