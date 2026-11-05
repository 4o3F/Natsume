//! One bounded login owner; GDM keeps ownership of workers and user desktops.

use super::{
    GDM, GreeterStatus, HELPER, LoginContext, PREPARE_TIMEOUT, mutation_when_available,
    role_observation,
};
use crate::{
    admission, gdm_registration, processes, runtime,
    session::{self, fresh_proxy, rejected, role_name, unavailable},
};
use natsume_workstation_ipc::{
    GraphicalSession, GraphicalSessionState, ResourceControlError, SessionRole,
};
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Stdio};
use tokio::{
    io::AsyncWriteExt as _,
    process::Command,
    time::{Duration, Instant, sleep, timeout},
};
use zbus::Connection;

const RECORD: &str = "session-preparation.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Greeter,
    Registration,
    Authentication,
    Desktop,
    Ready,
    Failed,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    boot: String,
    role: SessionRole,
    pid: i32,
    start: u64,
    phase: Phase,
    session: Option<GraphicalSession>,
    greeter: Option<GreeterStatus>,
}

impl State {
    fn record(&mut self, phase: Phase) -> Result<(), ResourceControlError> {
        if self.phase != phase {
            tracing::info!(
                role = role_name(self.role),
                ?phase,
                "Graphical preparation phase changed"
            );
            self.phase = phase;
        }
        runtime::write(Path::new("/"), RECORD, self)
    }
}

#[derive(Default)]
struct RegistrationObservation {
    stage: Option<&'static str>,
    kind: Option<&'static str>,
    entered: Option<Instant>,
    stage_started: Option<Instant>,
    finished: Option<Instant>,
}

impl RegistrationObservation {
    fn start_stage(&mut self, stage: &'static str, now: Instant) {
        self.stage = Some(stage);
        self.stage_started = Some(now);
    }

    fn enter(&mut self, now: Instant) {
        self.entered.get_or_insert(now);
    }

    fn finish(&mut self, now: Instant) {
        self.finished.get_or_insert(now);
    }

    fn timings(&self) -> (Option<u64>, Option<u64>) {
        let Some((entered, finished)) = self.entered.zip(self.finished) else {
            return (None, None);
        };
        (
            Some(crate::duration_micros(finished.duration_since(entered))),
            self.stage_started
                .map(|started| crate::duration_micros(finished.duration_since(started))),
        )
    }

    fn trace_incomplete(
        &self,
        role: SessionRole,
        phase: Phase,
        error: &ResourceControlError,
        elapsed: Duration,
    ) {
        let (registration_elapsed_us, registration_stage_elapsed_us) = self.timings();
        tracing::error!(%error, role = role_name(role), ?phase,
            elapsed_ms = elapsed.as_millis(),
            registration_stage = self.stage,
            registration_kind = self.kind.unwrap_or("not_sampled"),
            registration_elapsed_us, registration_stage_elapsed_us,
            "Graphical preparation incomplete; resample before retry");
    }

    fn trace_result(
        &self,
        role: SessionRole,
        elapsed: Duration,
        prepare_us: Option<Duration>,
        result: &Result<(), ResourceControlError>,
        timed_out: bool,
    ) {
        let (registration_elapsed_us, registration_stage_elapsed_us) = self.timings();
        tracing::info!(
            operation = "prepare",
            foreground_target = role_name(role),
            elapsed_us = crate::duration_micros(elapsed),
            prepare_us = prepare_us.map(crate::duration_micros),
            result = crate::local_result(result, timed_out),
            registration_stage = self.stage,
            registration_kind = self.kind.unwrap_or("not_sampled"),
            registration_elapsed_us,
            registration_stage_elapsed_us,
            "session switch completed"
        );
    }
}

pub(super) fn failed_session(
    root: &Path,
    role: SessionRole,
    current: Option<&GraphicalSession>,
) -> Result<bool, ResourceControlError> {
    let Some(current) = current else {
        return Ok(false);
    };
    let Some(state) = runtime::read::<State>(root, RECORD)? else {
        return Ok(false);
    };
    // A still-running desktop that failed preparation is not repeatedly pulled
    // to the front. Recovery of the UI or an explicit preparation can retry it.
    Ok(state.boot == current.boot_id
        && state.role == role
        && state.phase == Phase::Failed
        && state.session.as_ref() == Some(current))
}

async fn probe() -> Result<Option<GreeterStatus>, ResourceControlError> {
    let account = session::account(Path::new("/"), "gdm")?;
    let operation = Command::new("/usr/bin/setpriv")
        .args([
            "--reuid",
            &account.uid.to_string(),
            "--regid",
            &account.gid.to_string(),
            "--clear-groups",
            "--no-new-privs",
            HELPER,
            "greeter-status",
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let Ok(output) = timeout(Duration::from_secs(2), operation).await else {
        return Ok(None);
    };
    let output = output.map_err(|_| unavailable("fixed greeter probe cannot start"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let status: GreeterStatus = serde_json::from_slice(&output.stdout)
        .map_err(|_| rejected("fixed greeter probe returned invalid data"))?;
    if status.identity.uid != account.uid || status.identity.pid <= 1 || status.identity.start == 0
    {
        return Err(rejected("fixed greeter probe returned invalid identity"));
    }
    Ok(Some(status))
}

async fn registered_greeter(
    connection: &Connection,
    state: &mut State,
    registration: &mut RegistrationObservation,
) -> Result<LoginContext, ResourceControlError> {
    registration.start_stage("observer", Instant::now());
    gdm_registration::require_observer(Path::new("/"), connection).await?;
    registration.start_stage("greeter_lookup", Instant::now());
    if session::greeter(connection, Path::new("/"))
        .await?
        .is_none()
    {
        let factory = fresh_proxy(
            connection,
            GDM,
            "/org/gnome/DisplayManager/LocalDisplayFactory",
            "org.gnome.DisplayManager.LocalDisplayFactory",
        )
        .await?;
        // GDM owns this display. Its public interface cannot identify a native
        // Wayland display by session, so a missed acknowledgement never grants
        // permission to terminate or recycle an unproven greeter.
        let _display: zbus::OwnedObjectPath = factory.call("CreateTransientDisplay", &()).await?;
    }
    loop {
        registration.start_stage("greeter_lookup", Instant::now());
        let Some((id, path)) = session::greeter(connection, Path::new("/")).await? else {
            sleep(Duration::from_millis(100)).await;
            continue;
        };
        if state
            .greeter
            .as_ref()
            .is_some_and(|g| g.identity.session != id)
        {
            registration.kind = Some("greeter_replaced");
            return Err(unavailable(
                "captured greeter was replaced; resample before retry",
            ));
        }
        registration.start_stage("greeter_activation", Instant::now());
        session::activate_greeter(connection, &id, &path).await?;
        registration.enter(Instant::now());
        state.record(Phase::Registration)?;
        registration.start_stage("probe", Instant::now());
        if let Some(current) = probe().await.inspect_err(|_| {
            registration.kind = Some("probe_error");
        })? {
            check_greeter(state.greeter.as_ref(), &id, &current, registration)?;
            if state.greeter.is_none() {
                state.greeter = Some(current.clone());
                state.record(Phase::Registration)?;
            }
            registration.start_stage("proof", Instant::now());
            let lookup = gdm_registration::proof(Path::new("/"), connection, &id).await?;
            registration.kind = Some(lookup.kind());
            if let gdm_registration::ProofLookup::Available(proof) = lookup {
                check_proof(&proof, &current, registration)?;
                state.record(Phase::Authentication)?;
                let completed = Instant::now();
                registration.start_stage("complete", completed);
                registration.finish(completed);
                return Ok(LoginContext {
                    proof,
                    greeter: current,
                });
            }
        } else {
            registration.kind = Some("probe_unavailable");
        }
        sleep(Duration::from_millis(100)).await;
    }
}

fn check_greeter(
    captured: Option<&GreeterStatus>,
    session: &str,
    current: &GreeterStatus,
    registration: &mut RegistrationObservation,
) -> Result<(), ResourceControlError> {
    if current.identity.session != session || captured.is_some_and(|old| *old != *current) {
        registration.kind = Some("greeter_replaced");
        return Err(unavailable("captured greeter identity changed"));
    }
    Ok(())
}

fn check_proof(
    proof: &gdm_registration::Proof,
    current: &GreeterStatus,
    registration: &mut RegistrationObservation,
) -> Result<(), ResourceControlError> {
    if proof.greeter != current.identity {
        registration.kind = Some("proof_mismatch");
        return Err(rejected("GDM registration belongs to another greeter"));
    }
    Ok(())
}

async fn desktop(connection: &Connection, state: &mut State) -> Result<(), ResourceControlError> {
    state.record(Phase::Desktop)?;
    loop {
        let current = role_observation(
            session::observe(connection, Path::new("/")).await?,
            state.role,
        );
        if let Some(identity) = current.session {
            if state
                .session
                .as_ref()
                .is_some_and(|expected| *expected != identity)
            {
                return Err(rejected(
                    "graphical preparation cannot retarget a replacement session",
                ));
            }
            if state.session.is_none() {
                state.session = Some(identity.clone());
                state.record(Phase::Desktop)?;
            }
            if current.state == GraphicalSessionState::Running
                && !current.locked_hint
                && current.desktop_ready
            {
                // Native Wayland may defer application drawing while hidden.
                // Establish the foreground capability here; presentation is
                // validated again when the current target is applied.
                match session::activate(connection, Path::new("/"), state.role, &identity).await {
                    Ok(()) => {
                        state.record(Phase::Ready)?;
                        return Ok(());
                    }
                    // Display resume may temporarily withdraw a capability.
                    // Keep the captured identity and the existing outer deadline.
                    Err(ResourceControlError::Unavailable(_)) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        if matches!(
            current.state,
            GraphicalSessionState::Ambiguous | GraphicalSessionState::Error
        ) || current.locked_hint
        {
            return Err(rejected(
                "graphical preparation observed an invalid or locked role",
            ));
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn prepare(
    connection: &Connection,
    state: &mut State,
    registration: &mut RegistrationObservation,
) -> Result<(), ResourceControlError> {
    let current = role_observation(
        session::observe(connection, Path::new("/")).await?,
        state.role,
    );
    if current.session.is_some() {
        if current.state != GraphicalSessionState::Running || current.locked_hint {
            return Err(rejected("existing graphical role cannot be prepared"));
        }
        return desktop(connection, state).await;
    }
    if current.state != GraphicalSessionState::None {
        return Err(rejected("graphical role is not absent"));
    }
    if state.role == SessionRole::Contest {
        admission::require_no_workers(Path::new("/"))?;
    }
    let context = registered_greeter(connection, state, registration).await?;
    let account = session::account(Path::new("/"), "gdm")?;
    let input = serde_json::to_vec(&context)
        .map_err(|_| unavailable("captured GDM context cannot be encoded"))?;
    let mut child = Command::new("/usr/bin/setpriv")
        .args([
            "--reuid",
            &account.uid.to_string(),
            "--regid",
            &account.gid.to_string(),
            "--clear-groups",
            "--no-new-privs",
            HELPER,
            "gdm-login",
            role_name(state.role),
        ])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| unavailable("fixed GDM client could not start"))?;
    let mut pipe = child
        .stdin
        .take()
        .ok_or_else(|| unavailable("fixed GDM client input is unavailable"))?;
    pipe.write_all(&input)
        .await
        .map_err(|_| unavailable("captured GDM context could not be sent"))?;
    drop(pipe);
    tokio::select! {
        result = desktop(connection, state) => {
            let _killed = child.kill().await;
            let _reaped = child.wait().await;
            result
        }
        result = child.wait() => {
            let status = result.map_err(|_| unavailable("fixed GDM client status is unavailable"))?;
            if status.success() {
                desktop(connection, state).await
            } else {
                Err(unavailable("GDM authentication failed before desktop initialization"))
            }
        }
    }
}

async fn prepare_attempt(
    budget: Duration,
    work: impl std::future::Future<Output = Result<(), ResourceControlError>>,
) -> (Result<(), ResourceControlError>, bool) {
    match timeout(budget, work).await {
        Ok(result) => (result, false),
        Err(_) => (
            Err(unavailable(
                "preparation_timeout: registration or desktop initialization did not complete",
            )),
            true,
        ),
    }
}

pub(super) async fn run(role: SessionRole) -> Result<(), ResourceControlError> {
    let started = Instant::now();
    let mut prepare_us = None;
    let mut timed_out = false;
    let mut registration = RegistrationObservation::default();
    let result = async {
        if !rustix::process::geteuid().is_root() {
            return Err(rejected("session preparation requires root"));
        }
        let begun = Instant::now();
        let _mutation = mutation_when_available(Path::new("/")).await?;
        if role == SessionRole::Waiting {
            crate::waiting::require_login_allowed(Path::new("/"))?;
        }
        if role == SessionRole::Contest {
            admission::require_open(Path::new("/"))?;
            crate::home::require_template(Path::new("/"))?;
        }
        let connection = Connection::system()
            .await
            .map_err(|_| unavailable("system bus is unavailable"))?;
        let boot = session::read_boot_id(Path::new("/"))?;
        let pid = i32::try_from(std::process::id())
            .map_err(|_| unavailable("preparation PID is invalid"))?;
        let start = processes::start_time(Path::new("/"), pid)?
            .ok_or_else(|| unavailable("preparation process is unavailable"))?;
        let mut state = State {
            boot,
            role,
            pid,
            start,
            phase: Phase::Greeter,
            session: None,
            greeter: None,
        };
        state.record(Phase::Greeter)?;
        // Keep the owner alive during bounded cleanup; dropping the work future
        // alone does not prove that GDM cancelled an already-opened session.
        let work = PREPARE_TIMEOUT
            .saturating_sub(Duration::from_secs(5))
            .saturating_sub(begun.elapsed());
        let prepare_started = Instant::now();
        let (result, hit) =
            prepare_attempt(work, prepare(&connection, &mut state, &mut registration)).await;
        registration.finish(Instant::now());
        prepare_us = Some(prepare_started.elapsed());
        timed_out = hit;
        if let Err(error) = &result {
            registration.trace_incomplete(role, state.phase, error, begun.elapsed());
            state.record(Phase::Failed)?;
            if role == SessionRole::Contest {
                let cleanup = async {
                    let observed = session::observe(&connection, Path::new("/")).await?;
                    if observed.waiting.desktop_ready
                        && !observed.waiting.locked_hint
                        && let Some(waiting) = observed.waiting.session
                    {
                        session::activate(
                            &connection,
                            Path::new("/"),
                            SessionRole::Waiting,
                            &waiting,
                        )
                        .await?;
                    }
                    Ok::<_, ResourceControlError>(())
                };
                if !matches!(
                    timeout(PREPARE_TIMEOUT.saturating_sub(begun.elapsed()), cleanup).await,
                    Ok(Ok(()))
                ) {
                    tracing::warn!(
                        code = "waiting_restore_unconfirmed",
                        "Could not confirm waiting after preparation failure"
                    );
                }
            }
        }
        result
    }
    .await;
    let elapsed = started.elapsed();
    registration.trace_result(role, elapsed, prepare_us, &result, timed_out);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{Record, Value};
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    use tracing::{Event, Subscriber};
    use tracing_subscriber::{
        Layer,
        layer::{Context, SubscriberExt as _},
    };

    #[derive(Clone, Default)]
    struct RegistrationEvents(Arc<Mutex<Vec<Record>>>);

    impl<S: Subscriber> Layer<S> for RegistrationEvents {
        fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
            let mut record = Record {
                fields: BTreeMap::default(),
                level: *event.metadata().level(),
            };
            event.record(&mut record);
            self.0
                .lock()
                .unwrap_or_else(|_| panic!("registration event capture poisoned"))
                .push(record);
        }
    }

    fn greeter() -> GreeterStatus {
        GreeterStatus {
            identity: gdm_registration::Greeter {
                session: "registration-identity-canary".into(),
                uid: 107,
                pid: 4001,
                start: 12345,
            },
        }
    }

    fn proof(current: &GreeterStatus) -> gdm_registration::Proof {
        serde_json::from_value(serde_json::json!({
            "context": {"boot": "boot-canary", "bus": "bus-canary", "owner": "owner-canary"},
            "greeter": current.identity,
        }))
        .unwrap_or_else(|_| panic!("registration proof fixture is invalid"))
    }

    #[test]
    fn registration_timings_keep_the_phase_start_and_freeze_before_later_work() {
        let started = Instant::now();
        let mut observation = RegistrationObservation::default();
        observation.start_stage("observer", started);
        observation.finish(started + Duration::from_secs(1));
        assert_eq!(observation.timings(), (None, None));

        let mut observation = RegistrationObservation::default();
        observation.enter(started);
        observation.start_stage("probe", started);
        observation.enter(started + Duration::from_secs(4));
        observation.start_stage("proof", started + Duration::from_secs(10));
        observation.finish(started + Duration::from_secs(12));
        assert_eq!(observation.timings(), (Some(12_000_000), Some(2_000_000)));
        observation.finish(started + Duration::from_secs(20));
        assert_eq!(observation.timings(), (Some(12_000_000), Some(2_000_000)));

        let mut successful = RegistrationObservation::default();
        successful.enter(started);
        successful.start_stage("complete", started + Duration::from_secs(5));
        successful.finish(started + Duration::from_secs(5));
        successful.finish(started + Duration::from_secs(20));
        assert_eq!(successful.timings(), (Some(5_000_000), Some(0)));
    }

    #[test]
    fn greeter_and_proof_mismatches_keep_the_original_errors() {
        let current = greeter();
        let mut observation = RegistrationObservation::default();
        assert!(check_greeter(None, &current.identity.session, &current, &mut observation).is_ok());
        assert!(
            check_greeter(
                Some(&current),
                &current.identity.session,
                &current,
                &mut observation
            )
            .is_ok()
        );
        assert!(
            matches!(check_greeter(None, "replacement", &current, &mut observation),
            Err(ResourceControlError::Unavailable(message)) if message == "captured greeter identity changed")
        );
        assert_eq!(observation.kind, Some("greeter_replaced"));
        let mut captured = current.clone();
        captured.identity.start += 1;
        assert!(
            matches!(check_greeter(Some(&captured), &current.identity.session, &current, &mut observation),
            Err(ResourceControlError::Unavailable(message)) if message == "captured greeter identity changed")
        );
        assert_eq!(observation.kind, Some("greeter_replaced"));

        let mut proof = proof(&current);
        assert!(check_proof(&proof, &current, &mut observation).is_ok());
        proof.greeter.pid += 1;
        assert!(matches!(check_proof(&proof, &current, &mut observation),
            Err(ResourceControlError::Rejected(message)) if message == "GDM registration belongs to another greeter"));
        assert_eq!(observation.kind, Some("proof_mismatch"));
    }

    #[test]
    fn registration_events_share_frozen_fields_without_logging_identities_or_polls() {
        let _guard = crate::tests::subscriber_guard();
        let captured = RegistrationEvents::default();
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(captured.clone()),
            || {
                let started = Instant::now();
                let mut observation = RegistrationObservation::default();
                observation.enter(started);
                observation.start_stage("proof", started + Duration::from_secs(10));
                let current = greeter();
                let mut proof = proof(&current);
                proof.greeter.pid += 1;
                let result = check_proof(&proof, &current, &mut observation);
                observation.finish(started + Duration::from_secs(12));
                assert!(
                    captured
                        .0
                        .lock()
                        .unwrap_or_else(|_| panic!("event capture poisoned"))
                        .is_empty()
                );
                let Err(error) = &result else {
                    panic!("mismatched proof was accepted")
                };
                observation.trace_incomplete(
                    SessionRole::Contest,
                    Phase::Registration,
                    error,
                    Duration::from_secs(12),
                );
                observation.finish(started + Duration::from_secs(20));
                observation.trace_result(
                    SessionRole::Contest,
                    Duration::from_secs(20),
                    Some(Duration::from_secs(12)),
                    &result,
                    false,
                );
            },
        );
        let records = captured
            .0
            .lock()
            .unwrap_or_else(|_| panic!("event capture poisoned"));
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].level, tracing::Level::ERROR);
        assert_eq!(records[1].level, tracing::Level::INFO);
        assert_eq!(
            records[1].fields.get("result"),
            Some(&Value::Text("error".into()))
        );
        for record in records.iter() {
            for (field, expected) in [
                ("registration_stage", Value::Text("proof".into())),
                ("registration_kind", Value::Text("proof_mismatch".into())),
                ("registration_elapsed_us", Value::Unsigned(12_000_000)),
                ("registration_stage_elapsed_us", Value::Unsigned(2_000_000)),
            ] {
                assert_eq!(record.fields.get(field), Some(&expected), "field {field}");
            }
            let fields = format!("{:?}", record.fields);
            for canary in [
                "registration-identity-canary",
                "boot-canary",
                "bus-canary",
                "owner-canary",
                "4001",
                "12345",
            ] {
                assert!(!fields.contains(canary), "identity leaked: {canary}");
            }
        }
    }

    #[tokio::test]
    async fn preparation_attempt_keeps_result_and_identifies_only_its_own_timeout() {
        let (result, hit) = prepare_attempt(Duration::from_secs(40), async { Ok(()) }).await;
        assert!(result.is_ok());
        assert!(!hit);
        let (result, hit) =
            prepare_attempt(Duration::from_secs(40), async { Err(rejected("fixture")) }).await;
        assert!(matches!(result, Err(ResourceControlError::Rejected(_))));
        assert!(!hit);
        let mut observation = RegistrationObservation::default();
        let (result, hit) = prepare_attempt(Duration::from_millis(1), async {
            let started = Instant::now();
            observation.enter(started);
            observation.start_stage("proof", started);
            observation.kind = Some("proof_missing");
            std::future::pending().await
        })
        .await;
        observation.finish(Instant::now());
        assert!(hit);
        assert_eq!(observation.stage, Some("proof"));
        assert_eq!(observation.kind, Some("proof_missing"));
        assert!(observation.timings().0.is_some_and(|elapsed| elapsed > 0));
        assert!(
            matches!(&result, Err(ResourceControlError::Unavailable(message)) if message == "preparation_timeout: registration or desktop initialization did not complete")
        );
        assert_eq!(crate::local_result(&result, hit), "timeout");
    }

    #[tokio::test]
    async fn preparation_entry_failure_has_no_invented_prepare_interval() {
        use crate::tests::{Capture, Value, assert_text, subscriber_guard};
        use tracing::instrument::WithSubscriber as _;
        if rustix::process::geteuid().is_root() {
            return;
        }
        let _guard = subscriber_guard();
        let capture = Capture::default();
        async {
            assert!(run(SessionRole::Waiting).await.is_err());
            let events = capture.events("session switch completed");
            assert_eq!(events.len(), 1);
            assert_text(&events[0], "operation", "prepare");
            assert_text(&events[0], "foreground_target", "waiting");
            assert_text(&events[0], "result", "error");
            assert!(matches!(
                events[0].fields.get("elapsed_us"),
                Some(Value::Unsigned(_))
            ));
            assert!(!events[0].fields.contains_key("prepare_us"));
            assert!(!events[0].fields.contains_key("activate_us"));
            assert!(!events[0].fields.contains_key("registration_elapsed_us"));
            assert!(
                !events[0]
                    .fields
                    .contains_key("registration_stage_elapsed_us")
            );
        }
        .with_subscriber(capture.subscriber())
        .await;
    }
}
