use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use natsume_device_protocol::generated::{
    BindingAccessActualState, BindingAccessTarget, BindingArtifactState, BindingContext,
    BindingInput, BindingNegotiationIntent,
};
use natsume_device_protocol::is_valid_domjudge_username;
use natsume_local_control_api::{
    BindingSubmission, DEVICE1_PATH, DEVICE1_SERVICE, GraphicalSession, GraphicalSessionState,
    Privileged1Proxy, SessionAgentLease, SessionPresentation, SessionScreenKind, SessionUiSnapshot,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zbus::message::Header;

mod caller;
use zeroize::Zeroizing;

use crate::{
    atomic_write::{WritePolicy, atomic_write, durable_remove},
    canonical_uuid_v7,
};

use super::{
    SnapshotError,
    caddy::{CaddyModeArtifact, CaddyObservation},
    check_cancellation, invalid_epoch,
};

const INPUT_FORMAT_VERSION: u32 = 1;
const SEAT_CODE_LENGTH_LIMIT: usize = 64;

/// Durable Client decision for the current Binding negotiation.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BindingInputArtifact {
    format_version: u32,
    negotiation_id: String,
    submission_epoch: u64,
    seat_code: String,
}

/// Complete non-secret Binding context shared by the assignment artifact and Caddy mode marker.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ValidatedBindingContext {
    pub(super) binding_id: String,
    pub(super) account_id: String,
    pub(super) seat_code: String,
    pub(super) domjudge_username: String,
    pub(super) credential_revision: u64,
}

impl ValidatedBindingContext {
    fn from_wire(context: BindingContext) -> Option<Self> {
        let BindingContext {
            binding_id,
            account_id,
            seat_code,
            domjudge_username,
            credential_revision,
        } = context;
        Self {
            binding_id,
            account_id,
            seat_code,
            domjudge_username,
            credential_revision,
        }
        .validate()
    }

    fn validate(self) -> Option<Self> {
        (canonical_uuid_v7(&self.binding_id).is_some()
            && canonical_uuid_v7(&self.account_id).is_some()
            && valid_text(&self.seat_code, SEAT_CODE_LENGTH_LIMIT)
            && is_valid_domjudge_username(&self.domjudge_username)
            && self.credential_revision > 0
            && self.credential_revision <= i64::MAX.cast_unsigned())
        .then_some(self)
    }

    fn into_wire(self) -> BindingContext {
        BindingContext {
            binding_id: self.binding_id,
            account_id: self.account_id,
            seat_code: self.seat_code,
            domjudge_username: self.domjudge_username,
            credential_revision: self.credential_revision,
        }
    }
}

/// Binding negotiation intent accepted at the complete Server snapshot boundary.
#[derive(Clone, PartialEq)]
pub(super) struct ValidatedBindingIntent {
    negotiation_id: String,
    evaluation: Option<ValidatedBindingEvaluation>,
}

/// Server evaluation paired with the accepted Binding negotiation.
#[derive(Clone, PartialEq)]
struct ValidatedBindingEvaluation {
    submission_epoch: u64,
    error_code: String,
}

/// Bound access material accepted at the complete Server snapshot boundary.
#[derive(PartialEq)]
pub(super) struct ValidatedBoundTarget {
    pub(super) context: ValidatedBindingContext,
    pub(super) password: Zeroizing<String>,
}

/// Binding target accepted at the complete Server snapshot boundary.
#[derive(PartialEq)]
pub(super) struct ValidatedBindingTarget {
    pub(super) bound: Option<ValidatedBoundTarget>,
}

pub(super) fn validate_intent(intent: BindingNegotiationIntent) -> Option<ValidatedBindingIntent> {
    canonical_uuid_v7(&intent.negotiation_id)?;
    let evaluation = match intent.evaluation {
        None => None,
        Some(evaluation) => {
            if invalid_epoch(evaluation.submission_epoch)
                || !matches!(
                    evaluation.error_code.as_str(),
                    "SEAT_NOT_FOUND" | "SEAT_UNMAPPED" | "SEAT_OCCUPIED"
                )
            {
                return None;
            }
            Some(ValidatedBindingEvaluation {
                submission_epoch: evaluation.submission_epoch,
                error_code: evaluation.error_code,
            })
        }
    };
    Some(ValidatedBindingIntent {
        negotiation_id: intent.negotiation_id,
        evaluation,
    })
}

pub(super) fn validate_target(target: BindingAccessTarget) -> Option<ValidatedBindingTarget> {
    let bound = match target.bound {
        None => None,
        Some(bound) => {
            let context = ValidatedBindingContext::from_wire(bound.context?)?;
            let password = String::from_utf8(bound.password?.value).ok()?;
            if !valid_password(&password) {
                return None;
            }
            Some(ValidatedBoundTarget {
                context,
                password: Zeroizing::new(password),
            })
        }
    };
    Some(ValidatedBindingTarget { bound })
}

/// Binding input shared by snapshot reconciliation and the local Session Agent service.
///
/// The current Intent is kept only in memory for stale Session Agent fencing. The accepted seat
/// and epoch are persisted before the corresponding [`BindingInput`] can be returned.
pub(super) struct BindingInputProvider {
    input_path: PathBuf,
    state: Mutex<BindingInputState>,
    changed: Notify,
    registered: Mutex<Option<Registration>>,
}

/// In-memory Binding authority serialized with submission persistence.
struct BindingInputState {
    current_plan: Option<CancellationToken>,
    eligible: bool,
    current_intent: Option<ValidatedBindingIntent>,
    ui_revision: u64,
}

impl BindingInputState {
    fn advance_ui_revision(&mut self) {
        self.ui_revision = self.ui_revision.saturating_add(1);
    }
}

impl BindingInputProvider {
    pub(super) fn production() -> Self {
        Self {
            input_path: PathBuf::from("/var/lib/natsume/state/binding-input.json"),
            state: Mutex::new(BindingInputState {
                current_plan: None,
                eligible: false,
                current_intent: None,
                ui_revision: 1,
            }),
            changed: Notify::new(),
            registered: Mutex::new(None),
        }
    }

    pub(super) fn begin_plan(&self, plan: &CancellationToken) -> Result<(), SnapshotError> {
        let mut state = self.state.lock().map_err(|_| SnapshotError::Artifact)?;
        state.current_plan = Some(plan.clone());
        if state.eligible {
            state.eligible = false;
            state.advance_ui_revision();
        }
        Ok(())
    }

    /// An unchanged Target may renew its task fence without hiding the same UI.
    /// Changed Targets and transport loss still revoke input through begin/end.
    pub(super) fn refresh_plan(&self, plan: &CancellationToken) -> Result<(), SnapshotError> {
        let mut state = self.state.lock().map_err(|_| SnapshotError::Artifact)?;
        if state
            .current_plan
            .as_ref()
            .is_none_or(CancellationToken::is_cancelled)
        {
            return Err(SnapshotError::Cancelled);
        }
        state.current_plan = Some(plan.clone());
        Ok(())
    }

    pub(super) fn end_plan(&self) -> Result<(), SnapshotError> {
        let mut state = self.state.lock().map_err(|_| SnapshotError::Artifact)?;
        state.current_plan = None;
        if state.eligible {
            state.eligible = false;
            state.advance_ui_revision();
        }
        Ok(())
    }

    /// Observation may withdraw permission, including while a new plan is queued.
    pub(super) fn revoke_eligibility(&self) -> Result<bool, SnapshotError> {
        let mut state = self.state.lock().map_err(|_| SnapshotError::Artifact)?;
        let changed = state.eligible;
        if changed {
            state.eligible = false;
            state.advance_ui_revision();
        }
        Ok(changed)
    }

    pub(super) fn clear_intent(&self, plan: &CancellationToken) -> Result<(), SnapshotError> {
        let mut state = self.state.lock().map_err(|_| SnapshotError::Artifact)?;
        require_current_plan(&state, plan)?;
        if state.current_intent.take().is_some() && state.eligible {
            state.advance_ui_revision();
        }
        Ok(())
    }

    /// Persists one exact Session Agent confirmation before making it publishable.
    pub(super) fn submit(
        &self,
        negotiation_id: &str,
        submission_epoch: u64,
        seat_code: &str,
    ) -> Result<(), SnapshotError> {
        let mut state = self.state.lock().map_err(|_| SnapshotError::Artifact)?;
        let current = state
            .current_intent
            .as_ref()
            .filter(|_| {
                state
                    .current_plan
                    .as_ref()
                    .is_some_and(|plan| !plan.is_cancelled())
                    && state.eligible
            })
            .ok_or(SnapshotError::StaleLocalInput)?;
        let persisted = read_input_artifact(&self.input_path)
            .filter(|input| input.negotiation_id == current.negotiation_id);
        if current.negotiation_id != negotiation_id
            || next_submission_epoch(current, persisted.as_ref())? != submission_epoch
            || !valid_text(seat_code, SEAT_CODE_LENGTH_LIMIT)
        {
            return Err(SnapshotError::StaleLocalInput);
        }
        let artifact = BindingInputArtifact {
            format_version: INPUT_FORMAT_VERSION,
            negotiation_id: negotiation_id.to_owned(),
            submission_epoch,
            seat_code: seat_code.to_owned(),
        };
        let encoded = serde_json::to_vec(&artifact).map_err(|_| SnapshotError::Artifact)?;
        atomic_write(&self.input_path, &encoded, 0o600, WritePolicy::Replace)
            .map_err(|_| SnapshotError::Artifact)?;
        self.changed.notify_one();
        state.advance_ui_revision();
        Ok(())
    }

    pub(super) async fn waiting_ready(
        &self,
        connection: &zbus::Connection,
        session: &GraphicalSession,
    ) -> bool {
        self.live_agent(connection, session, true).await
    }

    pub(super) fn retire_waiting(
        &self,
        expected: Option<&GraphicalSession>,
    ) -> Result<(), SnapshotError> {
        self.revoke_eligibility()?;
        let mut registered = self
            .registered
            .lock()
            .map_err(|_| SnapshotError::Artifact)?;
        if registered
            .as_ref()
            .is_some_and(|r| expected.is_none() || Some(&r.lease.session) == expected)
        {
            *registered = None;
        }
        Ok(())
    }

    /// UI eligibility is independent of a frame for the newly selected screen.
    /// Submit and maintenance still require the exact current revision's frame.
    pub(super) async fn waiting_agent_alive(
        &self,
        connection: &zbus::Connection,
        session: &GraphicalSession,
    ) -> bool {
        self.live_agent(connection, session, false).await
    }

    async fn live_agent(
        &self,
        connection: &zbus::Connection,
        session: &GraphicalSession,
        require_frame: bool,
    ) -> bool {
        let Some(owner) = self.agent_owner(session, require_frame) else {
            return false;
        };
        let Ok(bus) = zbus::fdo::DBusProxy::new(connection).await else {
            return false;
        };
        let Ok(name) = zbus::names::BusName::try_from(owner.as_str()) else {
            return false;
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), bus.name_has_owner(name))
            .await
            .is_ok_and(|result| result.unwrap_or(false))
            && self.agent_owner(session, require_frame).as_deref() == Some(owner.as_str())
    }

    fn agent_owner(&self, session: &GraphicalSession, require_frame: bool) -> Option<String> {
        let registered = self.registered.lock().ok()?;
        let snapshot = self.ui_snapshot(session.clone()).ok()?;
        registered
            .as_ref()
            .filter(|r| {
                r.lease.session == *session
                    && r.lease.expires_at_unix_ms > unix_time_ms()
                    && (!require_frame
                        || r.presentation
                            .as_ref()
                            .is_some_and(|p| p.ui_revision == snapshot.ui_revision))
            })
            .map(|r| r.caller.sender.clone())
    }

    pub(super) async fn changed(&self) {
        self.changed.notified().await;
    }

    pub(super) fn set_eligible(
        &self,
        plan: &CancellationToken,
        eligible: bool,
    ) -> Result<(), SnapshotError> {
        let mut state = self.state.lock().map_err(|_| SnapshotError::Artifact)?;
        require_current_plan(&state, plan)?;
        if state.eligible != eligible {
            state.eligible = eligible;
            state.advance_ui_revision();
        }
        Ok(())
    }

    pub(super) fn current_input(
        &self,
        plan: &CancellationToken,
        intent: ValidatedBindingIntent,
    ) -> Result<Option<BindingInput>, SnapshotError> {
        let mut state = self.state.lock().map_err(|_| SnapshotError::Artifact)?;
        require_current_plan(&state, plan)?;
        if state.current_intent.as_ref() != Some(&intent) {
            state.current_intent = Some(intent);
            if state.eligible {
                state.advance_ui_revision();
            }
        }
        let intent = state
            .current_intent
            .as_ref()
            .ok_or(SnapshotError::Artifact)?;
        Ok(read_input_artifact(&self.input_path)
            .filter(|input| input.negotiation_id == intent.negotiation_id)
            .map(BindingInputArtifact::into_wire))
    }

    pub(super) fn observed_input(&self) -> Result<Option<BindingInput>, SnapshotError> {
        let state = self.state.lock().map_err(|_| SnapshotError::Artifact)?;
        Ok(state.current_intent.as_ref().and_then(|intent| {
            read_input_artifact(&self.input_path)
                .filter(|input| input.negotiation_id == intent.negotiation_id)
                .map(BindingInputArtifact::into_wire)
        }))
    }

    fn ui_snapshot(&self, session: GraphicalSession) -> Result<SessionUiSnapshot, SnapshotError> {
        let state = self.state.lock().map_err(|_| SnapshotError::Artifact)?;
        let Some(intent) = state.current_intent.as_ref().filter(|_| {
            state
                .current_plan
                .as_ref()
                .is_some_and(|plan| !plan.is_cancelled())
                && state.eligible
        }) else {
            return Ok(SessionUiSnapshot {
                session,
                ui_revision: state.ui_revision,
                screen: SessionScreenKind::Waiting,
                binding_error_code: None,
                negotiation_id: None,
                submission_epoch: None,
            });
        };
        let persisted = read_input_artifact(&self.input_path)
            .filter(|input| input.negotiation_id == intent.negotiation_id);
        let rejected = intent.evaluation.as_ref().is_some_and(|evaluation| {
            persisted
                .as_ref()
                .is_some_and(|input| input.submission_epoch == evaluation.submission_epoch)
        });
        let pending = persisted.is_some() && !rejected;
        let next_epoch = next_submission_epoch(intent, persisted.as_ref())?;
        Ok(SessionUiSnapshot {
            session,
            ui_revision: state.ui_revision,
            screen: if pending {
                SessionScreenKind::BindingPending
            } else {
                SessionScreenKind::BindingPrompt
            },
            binding_error_code: intent
                .evaluation
                .as_ref()
                .filter(|_| rejected)
                .map(|evaluation| evaluation.error_code.clone()),
            negotiation_id: (!pending).then(|| intent.negotiation_id.clone()),
            submission_epoch: (!pending).then_some(next_epoch),
        })
    }
}

fn require_current_plan(
    state: &BindingInputState,
    plan: &CancellationToken,
) -> Result<(), SnapshotError> {
    if !plan.is_cancelled() && state.current_plan.as_ref() == Some(plan) {
        Ok(())
    } else {
        Err(SnapshotError::Cancelled)
    }
}

fn next_submission_epoch(
    intent: &ValidatedBindingIntent,
    persisted: Option<&BindingInputArtifact>,
) -> Result<u64, SnapshotError> {
    let persisted_epoch = persisted.map_or(0, |input| input.submission_epoch);
    let evaluated_epoch = intent
        .evaluation
        .as_ref()
        .map_or(0, |value| value.submission_epoch);
    persisted_epoch
        .max(evaluated_epoch)
        .checked_add(1)
        .filter(|epoch| *epoch <= i64::MAX.cast_unsigned())
        .ok_or(SnapshotError::Artifact)
}

/// Binding access reconciler for the durable assignment and live Caddy credential state.
pub(super) struct BindingReconciler {
    assignment_path: PathBuf,
}

impl BindingReconciler {
    pub(super) fn production() -> Self {
        Self {
            assignment_path: PathBuf::from("/var/lib/natsume/state/binding-assignment.json"),
        }
    }

    pub(super) fn is_applied(&self, target: &ValidatedBindingTarget) -> bool {
        match (
            target.bound.as_ref(),
            read_assignment(&self.assignment_path),
        ) {
            (None, AssignmentRead::Absent) => true,
            (Some(bound), AssignmentRead::Applied(context)) => context == bound.context,
            _ => false,
        }
    }

    pub(super) fn reconcile(
        &self,
        target: &ValidatedBindingTarget,
        cancellation: &CancellationToken,
    ) -> Result<(), SnapshotError> {
        check_cancellation(cancellation)?;
        let Some(bound) = target.bound.as_ref() else {
            durable_remove(&self.assignment_path).map_err(|_| SnapshotError::Artifact)?;
            return Ok(());
        };
        if matches!(
            read_assignment(&self.assignment_path),
            AssignmentRead::Applied(context) if context == bound.context
        ) {
            return Ok(());
        }
        let encoded = serde_json::to_vec(&bound.context).map_err(|_| SnapshotError::Artifact)?;
        atomic_write(&self.assignment_path, &encoded, 0o600, WritePolicy::Replace)
            .map_err(|_| SnapshotError::Artifact)
    }

    pub(super) fn observe(&self, caddy: &CaddyObservation) -> BindingAccessActualState {
        let context = match read_assignment(&self.assignment_path) {
            AssignmentRead::Absent => return absent_actual(),
            AssignmentRead::Failed => return failed_actual(),
            AssignmentRead::Applied(context) => context,
        };
        let loaded = match caddy.mode.as_ref() {
            Some(CaddyModeArtifact::Ready { binding, .. }) => binding == &context,
            Some(CaddyModeArtifact::Blocked { .. }) | None => false,
        };
        if loaded {
            BindingAccessActualState {
                assignment_state: BindingArtifactState::Applied.into(),
                credential_state: BindingArtifactState::Applied.into(),
                context: Some(context.into_wire()),
            }
        } else {
            partial_actual()
        }
    }
}

/// Single Device1 service backed directly by the current Binding input.
struct Registration {
    lease: SessionAgentLease,
    caller: caller::Caller,
    presentation: Option<SessionPresentation>,
}

impl Registration {
    fn matches(&self, caller: &caller::Caller, lease_id: &str, session: &GraphicalSession) -> bool {
        &self.caller == caller
            && registration_matches(Some(&self.lease), lease_id, session, unix_time_ms())
    }

    fn confirm(
        &mut self,
        presentation: SessionPresentation,
        current_revision: u64,
    ) -> zbus::fdo::Result<()> {
        self.presentation = None;
        let dimensions_valid = if presentation.first_frame_presented {
            presentation.fullscreen_width > 0 && presentation.fullscreen_height > 0
        } else {
            presentation.fullscreen_width == 0 && presentation.fullscreen_height == 0
        };
        if presentation.ui_revision != current_revision || !dimensions_valid {
            return Err(service_error("Session presentation is incomplete or stale"));
        }
        // RandR may briefly resize the monitor before resizing the window.
        // Withdraw readiness during that transition without replacing the lease.
        self.presentation = presentation.first_frame_presented.then_some(presentation);
        Ok(())
    }
}

pub(super) struct DeviceService {
    provider: Arc<BindingInputProvider>,
    privileged_connection: zbus::Connection,
}

impl DeviceService {
    pub(super) async fn start(
        connection: &zbus::Connection,
        provider: Arc<BindingInputProvider>,
    ) -> Result<(), SnapshotError> {
        connection
            .object_server()
            .at(
                DEVICE1_PATH,
                Self {
                    provider,
                    privileged_connection: connection.clone(),
                },
            )
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        connection
            .request_name(DEVICE1_SERVICE)
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        Ok(())
    }

    async fn exact_current_session(&self, session: &GraphicalSession) -> bool {
        let Ok(proxy) = Privileged1Proxy::new(&self.privileged_connection).await else {
            return false;
        };
        matches!(
            proxy.query_managed_sessions().await,
            Ok(observation)
                if matches!(
                    observation.waiting.state,
                    GraphicalSessionState::Running
                ) && observation.waiting.session.as_ref() == Some(session)
        )
    }

    async fn authenticated(
        &self,
        header: &Header<'_>,
        session: &GraphicalSession,
    ) -> zbus::fdo::Result<caller::Caller> {
        let caller = caller::authenticate(&self.privileged_connection, header, session)
            .await
            .ok_or_else(|| service_error("Session Agent caller was rejected"))?;
        if !self.exact_current_session(session).await {
            return Err(service_error("Session Agent session is stale"));
        }
        Ok(caller)
    }
}

#[zbus::interface(name = "org.natsume.Device1")]
impl DeviceService {
    #[zbus(name = "RegisterSessionAgent")]
    async fn register_session_agent(
        &self,
        session: GraphicalSession,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<(SessionAgentLease, SessionUiSnapshot)> {
        let caller = self.authenticated(&header, &session).await?;
        let snapshot = self
            .provider
            .ui_snapshot(session.clone())
            .map_err(|_| service_error("Session UI state is unavailable"))?;
        let lease = SessionAgentLease {
            lease_id: Uuid::now_v7().hyphenated().to_string(),
            session,
            expires_at_unix_ms: unix_time_ms().saturating_add(15_000),
        };
        let mut registered = self
            .provider
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        *registered = Some(Registration {
            lease: lease.clone(),
            caller,
            presentation: None,
        });
        self.provider.changed.notify_one();
        Ok((lease, snapshot))
    }

    #[zbus(name = "RenewSessionAgentLease")]
    async fn renew_session_agent_lease(
        &self,
        lease_id: &str,
        session: GraphicalSession,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<SessionAgentLease> {
        let caller = self.authenticated(&header, &session).await?;
        let mut registered = self
            .provider
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        let registration = registered
            .as_mut()
            .filter(|registration| registration.matches(&caller, lease_id, &session))
            .ok_or_else(|| service_error("Session Agent lease is stale"))?;
        registration.lease.expires_at_unix_ms = unix_time_ms().saturating_add(15_000);
        Ok(registration.lease.clone())
    }

    #[zbus(name = "GetSessionUiSnapshot")]
    async fn get_session_ui_snapshot(
        &self,
        lease_id: &str,
        session: GraphicalSession,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<SessionUiSnapshot> {
        let caller = self.authenticated(&header, &session).await?;
        let registered = self
            .provider
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        if !registered
            .as_ref()
            .is_some_and(|r| r.matches(&caller, lease_id, &session))
        {
            return Err(service_error("Session Agent lease is stale"));
        }
        self.provider
            .ui_snapshot(session)
            .map_err(|_| service_error("Session UI state is unavailable"))
    }

    #[zbus(name = "ConfirmSessionPresentation")]
    async fn confirm_session_presentation(
        &self,
        lease_id: &str,
        presentation: SessionPresentation,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let caller = self.authenticated(&header, &presentation.session).await?;
        let mut registered = self
            .provider
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        let registration = registered
            .as_mut()
            .filter(|r| r.matches(&caller, lease_id, &presentation.session))
            .ok_or_else(|| service_error("Session Agent lease is stale"))?;
        let snapshot = self
            .provider
            .ui_snapshot(presentation.session.clone())
            .map_err(|_| service_error("Session UI state is unavailable"))?;
        let result = registration.confirm(presentation, snapshot.ui_revision);
        self.provider.changed.notify_one();
        result
    }

    #[zbus(name = "SubmitBinding")]
    async fn submit_binding(
        &self,
        lease_id: &str,
        submission: BindingSubmission,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let caller = self.authenticated(&header, &submission.session).await?;
        let proxy = Privileged1Proxy::new(&self.privileged_connection)
            .await
            .map_err(|_| service_error("waiting foreground is unavailable"))?;
        let observed = proxy
            .query_managed_sessions()
            .await
            .map_err(|_| service_error("waiting foreground is unavailable"))?;
        if !proxy.is_home_ready().await.unwrap_or(false)
            || observed.contest.state != GraphicalSessionState::Running
            || !observed.contest.desktop_ready
            || observed.contest.locked_hint
            || observed.foreground != natsume_local_control_api::SessionForeground::Waiting
            || observed.waiting.session.as_ref() != Some(&submission.session)
            || observed.waiting.locked_hint
            || !observed.waiting.desktop_ready
        {
            return Err(service_error(
                "Binding requires the ready waiting foreground",
            ));
        }
        let registered = self
            .provider
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        let snapshot = self
            .provider
            .ui_snapshot(submission.session.clone())
            .map_err(|_| service_error("Session UI state is unavailable"))?;
        if !registered.as_ref().is_some_and(|r| {
            r.matches(&caller, lease_id, &submission.session)
                && r.presentation
                    .as_ref()
                    .is_some_and(|p| p.ui_revision == snapshot.ui_revision)
        }) {
            return Err(service_error("Binding presentation is stale"));
        }
        let result = self.provider.submit(
            &submission.negotiation_id,
            submission.submission_epoch,
            &submission.seat_code,
        );
        drop(registered);
        result.map_err(|_| service_error("Binding submission was rejected"))
    }
}

impl BindingInputArtifact {
    fn into_wire(self) -> BindingInput {
        BindingInput {
            negotiation_id: self.negotiation_id,
            submission_epoch: self.submission_epoch,
            seat_code: self.seat_code,
        }
    }
}

fn read_input_artifact(path: &Path) -> Option<BindingInputArtifact> {
    let encoded = fs::read(path).ok()?;
    let artifact = serde_json::from_slice::<BindingInputArtifact>(&encoded).ok()?;
    (artifact.format_version == INPUT_FORMAT_VERSION
        && canonical_uuid_v7(&artifact.negotiation_id).is_some()
        && artifact.submission_epoch > 0
        && artifact.submission_epoch <= i64::MAX.cast_unsigned()
        && valid_text(&artifact.seat_code, SEAT_CODE_LENGTH_LIMIT))
    .then_some(artifact)
}

enum AssignmentRead {
    Absent,
    Applied(ValidatedBindingContext),
    Failed,
}

fn read_assignment(path: &Path) -> AssignmentRead {
    let encoded = match fs::read(path) {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return AssignmentRead::Absent;
        }
        Err(_) => return AssignmentRead::Failed,
    };
    let Some(context) = serde_json::from_slice::<ValidatedBindingContext>(&encoded)
        .ok()
        .and_then(ValidatedBindingContext::validate)
    else {
        return AssignmentRead::Failed;
    };
    AssignmentRead::Applied(context)
}

fn valid_text(value: &str, length_limit: usize) -> bool {
    !value.is_empty() && value.len() <= length_limit && !value.chars().any(char::is_control)
}

fn valid_password(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

fn registration_matches(
    registered: Option<&SessionAgentLease>,
    lease_id: &str,
    session: &GraphicalSession,
    now_unix_ms: i64,
) -> bool {
    registered.is_some_and(|registered| {
        registered.lease_id == lease_id
            && &registered.session == session
            && registered.expires_at_unix_ms > now_unix_ms
    })
}

fn service_error(message: &'static str) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(message.to_owned())
}

fn unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn absent_actual() -> BindingAccessActualState {
    BindingAccessActualState {
        assignment_state: BindingArtifactState::Absent.into(),
        credential_state: BindingArtifactState::Absent.into(),
        context: None,
    }
}

fn partial_actual() -> BindingAccessActualState {
    BindingAccessActualState {
        assignment_state: BindingArtifactState::Applied.into(),
        credential_state: BindingArtifactState::Failed.into(),
        context: None,
    }
}

fn failed_actual() -> BindingAccessActualState {
    BindingAccessActualState {
        assignment_state: BindingArtifactState::Failed.into(),
        credential_state: BindingArtifactState::Failed.into(),
        context: None,
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::{os::unix::fs::MetadataExt as _, sync::Barrier};

    use natsume_device_protocol::generated::{
        HomeActualState, HomeState, SessionControlActualState, SessionState,
    };
    use tempfile::TempDir;

    use super::*;

    fn tempdir() -> TempDir {
        TempDir::new().unwrap_or_else(|error| panic!("test directory must be created: {error}"))
    }

    pub(in crate::reconcile) fn input_provider(directory: &TempDir) -> BindingInputProvider {
        BindingInputProvider {
            input_path: directory.path().join("binding-input.json"),
            state: Mutex::new(BindingInputState {
                current_plan: None,
                eligible: false,
                current_intent: None,
                ui_revision: 1,
            }),
            changed: Notify::new(),
            registered: Mutex::new(None),
        }
    }

    pub(in crate::reconcile) fn confirm_fixture_frame(
        provider: &BindingInputProvider,
        sender: &str,
        session: GraphicalSession,
    ) {
        let snapshot = provider
            .ui_snapshot(session.clone())
            .unwrap_or_else(|e| panic!("UI: {e}"));
        let mut registered = provider
            .registered
            .lock()
            .unwrap_or_else(|e| panic!("Agent: {e}"));
        let registration = registered.get_or_insert_with(|| Registration {
            lease: SessionAgentLease {
                lease_id: "fixture-lease".to_owned(),
                session: session.clone(),
                expires_at_unix_ms: unix_time_ms() + 300_000,
            },
            caller: caller::tests::caller(sender),
            presentation: None,
        });
        if registration
            .presentation
            .as_ref()
            .is_none_or(|p| p.ui_revision != snapshot.ui_revision)
        {
            registration
                .confirm(
                    SessionPresentation {
                        session,
                        ui_revision: snapshot.ui_revision,
                        first_frame_presented: true,
                        fullscreen_width: 1280,
                        fullscreen_height: 800,
                    },
                    snapshot.ui_revision,
                )
                .unwrap_or_else(|e| panic!("frame: {e}"));
            provider.changed.notify_one();
        }
    }

    pub(in crate::reconcile) fn clear_fixture_agent(provider: &BindingInputProvider) {
        *provider
            .registered
            .lock()
            .unwrap_or_else(|e| panic!("Agent: {e}")) = None;
    }

    pub(in crate::reconcile) fn has_fixture_plan(provider: &BindingInputProvider) -> bool {
        provider
            .state
            .lock()
            .unwrap_or_else(|e| panic!("plan: {e}"))
            .current_plan
            .is_some()
    }

    pub(in crate::reconcile) fn reconciler(directory: &TempDir) -> BindingReconciler {
        BindingReconciler {
            assignment_path: directory.path().join("binding-assignment.json"),
        }
    }

    fn begin_plan(provider: &BindingInputProvider) -> CancellationToken {
        let plan = CancellationToken::new();
        provider
            .begin_plan(&plan)
            .unwrap_or_else(|error| panic!("plan must begin: {error}"));
        plan
    }

    fn validated_intent(negotiation_id: String) -> ValidatedBindingIntent {
        validate_intent(BindingNegotiationIntent {
            negotiation_id,
            evaluation: None,
        })
        .unwrap_or_else(|| panic!("test intent must validate"))
    }

    #[test]
    fn knowing_a_lease_does_not_allow_another_bus_connection_to_reuse_it() {
        let session = GraphicalSession {
            logind_session_id: "w1".to_owned(),
            boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        };
        let caller = caller::tests::caller(":1.8");
        let registration = Registration {
            lease: SessionAgentLease {
                lease_id: "lease".to_owned(),
                session: session.clone(),
                expires_at_unix_ms: unix_time_ms() + 15_000,
            },
            caller: caller.clone(),
            presentation: None,
        };
        assert!(registration.matches(&caller, "lease", &session));
        assert!(!registration.matches(&caller::tests::caller(":1.9"), "lease", &session));
    }

    #[test]
    fn resize_withdraws_the_frame_without_replacing_the_authenticated_lease() {
        let session = GraphicalSession {
            logind_session_id: "w1".to_owned(),
            boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        };
        let caller = caller::tests::caller(":1.8");
        let mut registration = Registration {
            lease: SessionAgentLease {
                lease_id: "lease".to_owned(),
                session: session.clone(),
                expires_at_unix_ms: unix_time_ms() + 15_000,
            },
            caller: caller.clone(),
            presentation: None,
        };
        let mut frame = SessionPresentation {
            session: session.clone(),
            ui_revision: 2,
            first_frame_presented: true,
            fullscreen_width: 1280,
            fullscreen_height: 800,
        };
        assert!(registration.confirm(frame.clone(), 2).is_ok());
        assert!(registration.presentation.is_some());
        frame.first_frame_presented = false;
        frame.fullscreen_width = 0;
        frame.fullscreen_height = 0;
        assert!(registration.confirm(frame.clone(), 2).is_ok());
        assert!(registration.presentation.is_none());
        assert!(registration.matches(&caller, "lease", &session));
        frame.first_frame_presented = true;
        frame.fullscreen_width = 1920;
        frame.fullscreen_height = 1080;
        assert!(registration.confirm(frame.clone(), 2).is_ok());
        assert!(registration.presentation.is_some());
        assert!(registration.confirm(frame.clone(), 3).is_err());
        assert!(registration.presentation.is_none());
        frame.fullscreen_width = 0;
        assert!(registration.confirm(frame, 2).is_err());
        assert!(registration.presentation.is_none());
    }

    #[test]
    fn binding_submission_is_persisted_before_it_is_returned() {
        let directory = tempdir();
        let provider = input_provider(&directory);
        let negotiation_id = Uuid::now_v7().hyphenated().to_string();
        let plan = begin_plan(&provider);
        provider
            .current_input(&plan, validated_intent(negotiation_id.clone()))
            .unwrap_or_else(|error| panic!("intent must be accepted: {error}"));
        provider
            .set_eligible(&plan, true)
            .unwrap_or_else(|error| panic!("submission must be enabled: {error}"));

        provider
            .submit(&negotiation_id, 1, "A-01")
            .unwrap_or_else(|error| panic!("submission must persist: {error}"));
        let persisted = read_input_artifact(&provider.input_path)
            .unwrap_or_else(|| panic!("accepted input must already be durable"));

        assert_eq!(persisted.negotiation_id, negotiation_id);
        assert_eq!(persisted.submission_epoch, 1);
        assert_eq!(persisted.seat_code, "A-01");
    }

    #[test]
    fn binding_assignment_never_contains_password_material() {
        let context = ValidatedBindingContext {
            binding_id: Uuid::now_v7().hyphenated().to_string(),
            account_id: Uuid::now_v7().hyphenated().to_string(),
            seat_code: "A-01".to_owned(),
            domjudge_username: "team-alpha".to_owned(),
            credential_revision: 1,
        };
        let encoded = serde_json::to_string(&context)
            .unwrap_or_else(|error| panic!("assignment must encode: {error}"));

        assert_eq!(
            serde_json::from_str::<ValidatedBindingContext>(&encoded)
                .unwrap_or_else(|error| panic!("assignment must decode: {error}")),
            context
        );
        assert!(!encoded.contains("password"));
    }

    #[test]
    fn exact_binding_target_does_not_replace_the_assignment() {
        let directory = tempdir();
        let reconciler = BindingReconciler {
            assignment_path: directory.path().join("binding-assignment.json"),
        };
        let target = ValidatedBindingTarget {
            bound: Some(ValidatedBoundTarget {
                context: ValidatedBindingContext {
                    binding_id: Uuid::now_v7().hyphenated().to_string(),
                    account_id: Uuid::now_v7().hyphenated().to_string(),
                    seat_code: "A-01".to_owned(),
                    domjudge_username: "team-alpha".to_owned(),
                    credential_revision: 1,
                },
                password: Zeroizing::new("password".to_owned()),
            }),
        };
        reconciler
            .reconcile(&target, &CancellationToken::new())
            .unwrap_or_else(|error| panic!("target must reconcile: {error}"));
        let inode = fs::metadata(&reconciler.assignment_path)
            .unwrap_or_else(|error| panic!("assignment metadata must load: {error}"))
            .ino();

        reconciler
            .reconcile(&target, &CancellationToken::new())
            .unwrap_or_else(|error| panic!("target replay must reconcile: {error}"));

        assert_eq!(
            fs::metadata(&reconciler.assignment_path)
                .unwrap_or_else(|error| panic!("assignment metadata must reload: {error}"))
                .ino(),
            inode
        );
    }

    #[test]
    fn corrupt_binding_assignment_is_failed_not_absent() {
        let directory = tempdir();
        let path = directory.path().join("binding-assignment.json");
        fs::write(&path, b"not-json")
            .unwrap_or_else(|error| panic!("assignment fixture must be written: {error}"));
        let reconciler = BindingReconciler {
            assignment_path: path,
        };
        let caddy = CaddyObservation {
            mode: None,
            gateway_leaf_sha256: None,
        };

        let actual = reconciler.observe(&caddy);
        assert_eq!(
            actual.assignment_state,
            i32::from(BindingArtifactState::Failed)
        );
        assert_eq!(
            actual.credential_state,
            i32::from(BindingArtifactState::Failed)
        );
        assert!(actual.context.is_none());
    }

    #[test]
    fn binding_prompt_requires_local_eligibility() {
        let directory = tempdir();
        let provider = input_provider(&directory);
        let plan = begin_plan(&provider);
        provider
            .current_input(
                &plan,
                validated_intent(Uuid::now_v7().hyphenated().to_string()),
            )
            .unwrap_or_else(|error| panic!("intent must be accepted: {error}"));
        let session = GraphicalSession {
            logind_session_id: "c2".to_owned(),
            boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        };

        let ineligible = provider
            .ui_snapshot(session.clone())
            .unwrap_or_else(|error| panic!("UI snapshot must be available: {error}"));
        provider
            .set_eligible(&plan, true)
            .unwrap_or_else(|error| panic!("prompt must be enabled: {error}"));
        let eligible = provider
            .ui_snapshot(session)
            .unwrap_or_else(|error| panic!("UI snapshot must be available: {error}"));

        assert_eq!(ineligible.screen, SessionScreenKind::Waiting);
        assert_eq!(eligible.screen, SessionScreenKind::BindingPrompt);
    }

    #[test]
    fn duplicate_submission_epoch_has_one_durable_winner() {
        let directory = tempdir();
        let provider = Arc::new(input_provider(&directory));
        let negotiation_id = Uuid::now_v7().hyphenated().to_string();
        let plan = begin_plan(&provider);
        provider
            .current_input(&plan, validated_intent(negotiation_id.clone()))
            .unwrap_or_else(|error| panic!("intent must be accepted: {error}"));
        provider
            .set_eligible(&plan, true)
            .unwrap_or_else(|error| panic!("submission must be enabled: {error}"));

        let attempts = ["A-01", "A-02"].map(|seat_code| {
            let provider = Arc::clone(&provider);
            let negotiation_id = negotiation_id.clone();
            std::thread::spawn(move || provider.submit(&negotiation_id, 1, seat_code).is_ok())
        });
        let accepted = attempts
            .into_iter()
            .map(|attempt| {
                attempt
                    .join()
                    .unwrap_or_else(|_| panic!("submission thread must finish"))
            })
            .filter(|accepted| *accepted)
            .count();

        assert_eq!(accepted, 1);
        assert_eq!(
            read_input_artifact(&provider.input_path).map(|input| input.submission_epoch),
            Some(1)
        );
    }

    #[test]
    fn unchanged_plan_refresh_preserves_frame_and_changed_plan_revokes_input()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir();
        let provider = input_provider(&directory);
        let plan = begin_plan(&provider);
        provider.current_input(&plan, validated_intent(Uuid::now_v7().to_string()))?;
        let session = GraphicalSession {
            logind_session_id: "w1".to_owned(),
            boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        };
        provider.set_eligible(&plan, true)?;
        confirm_fixture_frame(&provider, ":1.8", session.clone());
        let revision = provider.ui_snapshot(session.clone())?.ui_revision;
        let refreshed = CancellationToken::new();
        provider.refresh_plan(&refreshed)?;
        assert_eq!(provider.ui_snapshot(session.clone())?.ui_revision, revision);
        assert!(provider.agent_owner(&session, true).is_some());
        assert!(matches!(
            provider.set_eligible(&plan, true),
            Err(SnapshotError::Cancelled)
        ));
        provider.begin_plan(&CancellationToken::new())?;
        assert_eq!(
            provider.ui_snapshot(session.clone())?.screen,
            SessionScreenKind::Waiting
        );
        assert!(provider.agent_owner(&session, true).is_none());
        Ok(())
    }

    #[test]
    fn ineligible_binding_rejects_submission_without_writing() {
        let directory = tempdir();
        let provider = input_provider(&directory);
        let negotiation_id = Uuid::now_v7().hyphenated().to_string();
        let plan = begin_plan(&provider);
        provider
            .current_input(&plan, validated_intent(negotiation_id.clone()))
            .unwrap_or_else(|error| panic!("intent must be accepted: {error}"));
        provider
            .set_eligible(&plan, true)
            .unwrap_or_else(|error| panic!("submission must be enabled: {error}"));
        provider
            .set_eligible(&plan, false)
            .unwrap_or_else(|error| panic!("submission must be disabled: {error}"));

        let result = provider.submit(&negotiation_id, 1, "A-01");

        assert!(matches!(result, Err(SnapshotError::StaleLocalInput)));
        assert!(!provider.input_path.exists());
    }

    #[test]
    fn replacement_plan_cannot_reopen_previous_binding_authority() {
        let directory = tempdir();
        let provider = input_provider(&directory);
        let negotiation_id = Uuid::now_v7().hyphenated().to_string();
        let previous = begin_plan(&provider);
        provider
            .current_input(&previous, validated_intent(negotiation_id.clone()))
            .unwrap_or_else(|error| panic!("intent must be accepted: {error}"));
        provider
            .set_eligible(&previous, true)
            .unwrap_or_else(|error| panic!("submission must be enabled: {error}"));

        let _current = begin_plan(&provider);
        let reopen = provider.set_eligible(&previous, true);
        let submission = provider.submit(&negotiation_id, 1, "A-01");

        assert!(matches!(reopen, Err(SnapshotError::Cancelled)));
        assert!(matches!(submission, Err(SnapshotError::StaleLocalInput)));
        assert!(!provider.input_path.exists());
    }

    #[test]
    fn terminate_replacement_rejects_a_queued_submission() {
        let directory = tempdir();
        let provider = Arc::new(input_provider(&directory));
        let negotiation_id = Uuid::now_v7().hyphenated().to_string();
        let previous = begin_plan(&provider);
        provider
            .current_input(&previous, validated_intent(negotiation_id.clone()))
            .unwrap_or_else(|error| panic!("intent must be accepted: {error}"));
        provider
            .set_eligible(&previous, true)
            .unwrap_or_else(|error| panic!("submission must be enabled: {error}"));

        let session_check_passed = Arc::new(Barrier::new(2));
        let continue_submission = Arc::new(Barrier::new(2));
        let queued = {
            let provider = Arc::clone(&provider);
            let negotiation_id = negotiation_id.clone();
            let session_check_passed = Arc::clone(&session_check_passed);
            let continue_submission = Arc::clone(&continue_submission);
            std::thread::spawn(move || {
                session_check_passed.wait();
                continue_submission.wait();
                provider.submit(&negotiation_id, 1, "A-01")
            })
        };
        session_check_passed.wait();

        let replacement = begin_plan(&provider);
        provider
            .current_input(&replacement, validated_intent(negotiation_id))
            .unwrap_or_else(|error| panic!("replacement intent must be accepted: {error}"));
        let eligible = super::super::local_access_is_allowed(
            &super::super::validate_server_snapshot(super::super::tests::snapshot())
                .unwrap_or_else(|error| panic!("fixture target: {error}")),
            &SessionControlActualState {
                contest_ready: true,
                waiting_ready: false,
                foreground: natsume_device_protocol::generated::SessionForeground::Contest.into(),
                session_state: SessionState::None.into(),
                completed_terminate_epoch: Some(1),
            },
            &HomeActualState {
                state: HomeState::Steady.into(),
                completed_reset_epoch: None,
            },
        );
        provider
            .set_eligible(&replacement, eligible)
            .unwrap_or_else(|error| panic!("replacement eligibility must be set: {error}"));
        continue_submission.wait();

        let result = queued
            .join()
            .unwrap_or_else(|_| panic!("queued submission must finish"));
        assert!(matches!(result, Err(SnapshotError::StaleLocalInput)));
        assert!(!provider.input_path.exists());
    }

    #[test]
    fn expired_or_replaced_agent_lease_is_stale() {
        let session = GraphicalSession {
            logind_session_id: "c2".to_owned(),
            boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        };
        let registered = SessionAgentLease {
            lease_id: "current".to_owned(),
            session: session.clone(),
            expires_at_unix_ms: 10,
        };

        assert!(registration_matches(
            Some(&registered),
            "current",
            &session,
            9
        ));
        assert!(!registration_matches(
            Some(&registered),
            "current",
            &session,
            10
        ));
        assert!(!registration_matches(
            Some(&registered),
            "replaced",
            &session,
            9
        ));
    }

    #[test]
    fn domjudge_username_uses_the_import_contract_on_wire_and_disk() {
        let directory = tempdir();
        let assignment_path = directory.path().join("binding-assignment.json");
        let context = |username: String| BindingContext {
            binding_id: Uuid::now_v7().hyphenated().to_string(),
            account_id: Uuid::now_v7().hyphenated().to_string(),
            seat_code: "A-01".to_owned(),
            domjudge_username: username,
            credential_revision: 1,
        };

        for (username, accepted) in [
            ("Team_1.test+contest@example.org".to_owned(), true),
            ("u".repeat(64), true),
            ("u".repeat(65), false),
            (
                "{$NATSUME_UNSET_FOR_REVIEW:literal_changed}".to_owned(),
                false,
            ),
            ("{http.request.host}".to_owned(), false),
            ("team$1".to_owned(), false),
            ("team\\1".to_owned(), false),
            ("team\"1".to_owned(), false),
            ("team 1".to_owned(), false),
            ("队伍一".to_owned(), false),
        ] {
            let wire = context(username);
            let persisted = ValidatedBindingContext {
                binding_id: wire.binding_id.clone(),
                account_id: wire.account_id.clone(),
                seat_code: wire.seat_code.clone(),
                domjudge_username: wire.domjudge_username.clone(),
                credential_revision: wire.credential_revision,
            };
            let encoded = serde_json::to_vec(&persisted)
                .unwrap_or_else(|error| panic!("assignment must encode: {error}"));
            fs::write(&assignment_path, encoded)
                .unwrap_or_else(|error| panic!("assignment must be written: {error}"));
            assert_eq!(ValidatedBindingContext::from_wire(wire).is_some(), accepted);
            assert_eq!(
                matches!(
                    read_assignment(&assignment_path),
                    AssignmentRead::Applied(_)
                ),
                accepted
            );
        }
    }
}
