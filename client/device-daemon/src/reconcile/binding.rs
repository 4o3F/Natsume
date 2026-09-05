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
use natsume_local_control_api::{
    BindingSubmission, ContestSessionState, DEVICE1_PATH, DEVICE1_SERVICE, GraphicalSession,
    Privileged1Proxy, SessionAgentLease, SessionScreenKind, SessionUiSnapshot,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zbus::{Proxy, message::Header, zvariant::OwnedObjectPath};
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
const USERNAME_LENGTH_LIMIT: usize = 128;
const LOGIN1_SERVICE: &str = "org.freedesktop.login1";
const LOGIN1_MANAGER_PATH: &str = "/org/freedesktop/login1";
const LOGIN1_MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const LOGIN1_SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";

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
            && valid_text(&self.domjudge_username, USERNAME_LENGTH_LIMIT)
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

    pub(super) fn end_plan(&self) -> Result<(), SnapshotError> {
        let mut state = self.state.lock().map_err(|_| SnapshotError::Artifact)?;
        state.current_plan = None;
        if state.eligible {
            state.eligible = false;
            state.advance_ui_revision();
        }
        Ok(())
    }

    pub(super) fn clear_intent(&self, plan: &CancellationToken) -> Result<(), SnapshotError> {
        let mut state = self.state.lock().map_err(|_| SnapshotError::Artifact)?;
        require_current_plan(&state, plan)?;
        if state.current_intent.take().is_some() {
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
            .filter(|_| state.current_plan.is_some() && state.eligible)
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
            state.advance_ui_revision();
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
        let Some(intent) = state
            .current_intent
            .as_ref()
            .filter(|_| state.current_plan.is_some() && state.eligible)
        else {
            return Ok(SessionUiSnapshot {
                session,
                ui_revision: state.ui_revision,
                screen: SessionScreenKind::Hidden,
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
pub(super) struct DeviceService {
    provider: Arc<BindingInputProvider>,
    privileged_connection: zbus::Connection,
    registered: Mutex<Option<SessionAgentLease>>,
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
                    registered: Mutex::new(None),
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
            proxy.query_contest_session().await,
            Ok(observation)
                if matches!(
                    observation.state,
                    ContestSessionState::Active | ContestSessionState::Locked
                ) && observation.session.as_ref() == Some(session)
        )
    }

    async fn caller_matches_session(
        &self,
        header: &Header<'_>,
        claimed: &GraphicalSession,
    ) -> bool {
        let Some(sender) = header.sender() else {
            return false;
        };
        let Ok(bus) = zbus::fdo::DBusProxy::new(&self.privileged_connection).await else {
            return false;
        };
        let Ok(pid) = bus
            .get_connection_unix_process_id(sender.clone().into())
            .await
        else {
            return false;
        };
        let Ok(manager) = Proxy::new(
            &self.privileged_connection,
            LOGIN1_SERVICE,
            LOGIN1_MANAGER_PATH,
            LOGIN1_MANAGER_INTERFACE,
        )
        .await
        else {
            return false;
        };
        let Ok(path) = manager
            .call::<_, _, OwnedObjectPath>("GetSessionByPID", &(pid,))
            .await
        else {
            return false;
        };
        let Ok(session) = Proxy::new(
            &self.privileged_connection,
            LOGIN1_SERVICE,
            path.as_str(),
            LOGIN1_SESSION_INTERFACE,
        )
        .await
        else {
            return false;
        };
        matches!(session.get_property::<String>("Id").await, Ok(id) if id == claimed.logind_session_id)
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
        if !self.exact_current_session(&session).await
            || !self.caller_matches_session(&header, &session).await
        {
            return Err(service_error("Session Agent registration was rejected"));
        }
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
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        *registered = Some(lease.clone());
        Ok((lease, snapshot))
    }

    #[zbus(name = "RenewSessionAgentLease")]
    async fn renew_session_agent_lease(
        &self,
        lease_id: &str,
        session: GraphicalSession,
    ) -> zbus::fdo::Result<SessionAgentLease> {
        if !self.exact_current_session(&session).await {
            return Err(service_error("Session Agent lease is stale"));
        }
        let now_unix_ms = unix_time_ms();
        let expires_at_unix_ms = now_unix_ms.saturating_add(15_000);
        let mut registered = self
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        let Some(registered) = registered.as_mut() else {
            return Err(service_error("Session Agent lease is stale"));
        };
        if !registration_matches(Some(registered), lease_id, &session, now_unix_ms) {
            return Err(service_error("Session Agent lease is stale"));
        }
        registered.expires_at_unix_ms = expires_at_unix_ms;
        Ok(registered.clone())
    }

    #[zbus(name = "GetSessionUiSnapshot")]
    fn get_session_ui_snapshot(
        &self,
        lease_id: &str,
        session: GraphicalSession,
    ) -> zbus::fdo::Result<SessionUiSnapshot> {
        let registered = self
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        if !registration_matches(registered.as_ref(), lease_id, &session, unix_time_ms()) {
            return Err(service_error("Session Agent lease is stale"));
        }
        self.provider
            .ui_snapshot(session)
            .map_err(|_| service_error("Session UI state is unavailable"))
    }

    #[zbus(name = "SubmitBinding")]
    async fn submit_binding(
        &self,
        lease_id: &str,
        submission: BindingSubmission,
    ) -> zbus::fdo::Result<()> {
        if !self.exact_current_session(&submission.session).await {
            return Err(service_error("Binding submission session is stale"));
        }
        let registered = self
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        if !registration_matches(
            registered.as_ref(),
            lease_id,
            &submission.session,
            unix_time_ms(),
        ) {
            return Err(service_error("Binding submission session is stale"));
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
mod tests {
    use std::{os::unix::fs::MetadataExt as _, sync::Barrier};

    use natsume_device_protocol::generated::{
        HomeActualState, HomeState, SessionControlActualState, SessionState,
    };
    use tempfile::TempDir;

    use super::*;

    fn tempdir() -> TempDir {
        TempDir::new().unwrap_or_else(|error| panic!("test directory must be created: {error}"))
    }

    fn input_provider(directory: &TempDir) -> BindingInputProvider {
        BindingInputProvider {
            input_path: directory.path().join("binding-input.json"),
            state: Mutex::new(BindingInputState {
                current_plan: None,
                eligible: false,
                current_intent: None,
                ui_revision: 1,
            }),
            changed: Notify::new(),
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

        assert_eq!(ineligible.screen, SessionScreenKind::Hidden);
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
        let eligible = super::super::binding_input_is_eligible(
            &ValidatedBindingTarget { bound: None },
            &SessionControlActualState {
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
    fn domjudge_username_uses_the_server_length_contract() {
        let context = |username: String| BindingContext {
            binding_id: Uuid::now_v7().hyphenated().to_string(),
            account_id: Uuid::now_v7().hyphenated().to_string(),
            seat_code: "A-01".to_owned(),
            domjudge_username: username,
            credential_revision: 1,
        };

        assert!(ValidatedBindingContext::from_wire(context("u".repeat(65))).is_some());
        assert!(ValidatedBindingContext::from_wire(context("u".repeat(128))).is_some());
        assert!(ValidatedBindingContext::from_wire(context("u".repeat(129))).is_none());
    }
}
