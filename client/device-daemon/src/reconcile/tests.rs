use natsume_device_protocol::generated::{
    BindingAccessTarget, BindingNegotiationIntent, ConcreteTargetState, ForegroundTarget,
    GatewayCredentialIntent, GatewayTarget, HomeActualState, HomeState, HomeTarget,
    RuntimeConfigActualState, RuntimeConfigState, RuntimeConfigTarget, ServerIntentState,
    SessionControlActualState, SessionControlTarget, SessionForeground as WireForeground,
    SessionState,
};
use uuid::Uuid;

use super::{access::applied_runtime_origin, *};

#[test]
fn helper_retry_policy_uses_error_types_and_rejects_unknown_failures() {
    assert!(retryable_control_error(&ResourceControlError::Unavailable(
        "same text".to_owned()
    )));
    assert!(!retryable_control_error(&ResourceControlError::Rejected(
        "same text".to_owned()
    )));
    assert!(retryable_control_error(&ResourceControlError::ZBus(
        zbus::fdo::Error::NoReply("timeout".to_owned()).into()
    )));
    assert!(!retryable_control_error(&ResourceControlError::ZBus(
        zbus::fdo::Error::AccessDenied("denied".to_owned()).into()
    )));
    assert!(!retryable_control_error(&ResourceControlError::ZBus(
        zbus::Error::Failure("unknown".to_owned())
    )));
}

use natsume_local_control_api::{
    GraphicalSession, GraphicalSessionObservation, GraphicalSessionState, HomeResetPhase,
    HomeResetProgress, ManagedSessionsObservation, PRIVILEGED1_PATH, ResourceControlError,
    SessionForeground,
};
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct HelperState {
    pub(crate) session_state: Option<GraphicalSessionState>,
    pub(crate) foreground: Option<SessionForeground>,
    pub(crate) session_after_home: Option<GraphicalSessionState>,
    pub(crate) require_blocked: Option<std::path::PathBuf>,
    pub(crate) termination_calls: Vec<GraphicalSession>,
    pub(crate) home_calls: Vec<u64>,
    pub(crate) terminate_failures: usize,
    pub(crate) home_failures: usize,
    pub(crate) rejected: bool,
    pub(crate) progress: Option<HomeResetProgress>,
    pub(crate) activation_calls: Vec<(natsume_local_control_api::SessionRole, GraphicalSession)>,
    pub(crate) preparation_calls: Vec<natsume_local_control_api::SessionRole>,
    pub(crate) waiting_resume_calls: usize,
    pub(crate) waiting_recovery_calls: Vec<Option<GraphicalSession>>,
}

struct FaultyHelper(Arc<Mutex<HelperState>>);

#[zbus::interface(name = "org.natsume.Privileged1")]
impl FaultyHelper {
    #[zbus(name = "ResumeWaitingRecovery")]
    fn resume_waiting_recovery(&mut self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| panic!("fixture: {e}"))
            .waiting_resume_calls += 1;
        false
    }

    #[zbus(name = "RecoverWaitingSession")]
    fn recover_waiting_session(&mut self, expected: Option<GraphicalSession>) -> bool {
        assert_eq!(self.query_managed_sessions().waiting.session, expected);
        self.0
            .lock()
            .unwrap_or_else(|e| panic!("fixture: {e}"))
            .waiting_recovery_calls
            .push(expected);
        false
    }

    #[zbus(name = "PrepareBootSessions")]
    fn prepare_boot_sessions(&mut self, waiting: GraphicalSession) -> bool {
        assert_eq!(self.query_managed_sessions().waiting.session, Some(waiting));
        assert!(self.is_home_ready());
        // These fixtures begin after the fixed boot preparation barrier.
        true
    }

    #[zbus(name = "PrepareSession")]
    fn prepare_session(
        &mut self,
        role: natsume_local_control_api::SessionRole,
    ) -> GraphicalSessionObservation {
        assert!(self.is_home_ready());
        self.0
            .lock()
            .unwrap_or_else(|e| panic!("fixture: {e}"))
            .preparation_calls
            .push(role);
        match role {
            natsume_local_control_api::SessionRole::Waiting => {
                self.query_managed_sessions().waiting
            }
            natsume_local_control_api::SessionRole::Contest => {
                self.query_managed_sessions().contest
            }
        }
    }

    #[zbus(name = "ActivateSession")]
    fn activate_session(
        &mut self,
        role: natsume_local_control_api::SessionRole,
        session: GraphicalSession,
    ) {
        let observed = self.query_managed_sessions();
        let (expected, foreground) = match role {
            natsume_local_control_api::SessionRole::Waiting => {
                (observed.waiting, SessionForeground::Waiting)
            }
            natsume_local_control_api::SessionRole::Contest => {
                (observed.contest, SessionForeground::Contest)
            }
        };
        assert_eq!(expected.session, Some(session.clone()));
        assert!(expected.desktop_ready && !expected.locked_hint);
        let mut state = self.0.lock().unwrap_or_else(|e| panic!("fixture: {e}"));
        state.activation_calls.push((role, session));
        state.foreground = Some(foreground);
    }

    #[zbus(name = "QueryManagedSessions")]
    fn query_managed_sessions(&self) -> ManagedSessionsObservation {
        let state = self
            .0
            .lock()
            .unwrap_or_else(|e| panic!("fixture lock: {e}"));
        let lifecycle = state
            .session_state
            .unwrap_or(GraphicalSessionState::Running);
        ManagedSessionsObservation {
            contest: GraphicalSessionObservation {
                state: lifecycle,
                session: matches!(
                    lifecycle,
                    GraphicalSessionState::Running
                        | GraphicalSessionState::Starting
                        | GraphicalSessionState::Terminating
                )
                .then(|| GraphicalSession {
                    logind_session_id: if state.termination_calls.is_empty() {
                        "c2"
                    } else {
                        "c3"
                    }
                    .to_owned(),
                    boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
                }),
                desktop_ready: lifecycle == GraphicalSessionState::Running,
                locked_hint: false,
            },
            waiting: GraphicalSessionObservation {
                state: GraphicalSessionState::Running,
                session: Some(GraphicalSession {
                    logind_session_id: "w1".to_owned(),
                    boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
                }),
                desktop_ready: true,
                locked_hint: false,
            },
            foreground: state.foreground.unwrap_or(SessionForeground::Contest),
        }
    }

    #[zbus(name = "TerminateContestSession")]
    fn terminate_contest_session(
        &mut self,
        session: GraphicalSession,
    ) -> Result<(), ResourceControlError> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|e| panic!("fixture lock: {e}"));
        state.assert_blocked();
        state.termination_calls.push(session);
        if state.rejected {
            Err(ResourceControlError::Rejected(
                "invalid session target".to_owned(),
            ))
        } else if state.termination_calls.len() <= state.terminate_failures {
            Err(ResourceControlError::Unavailable(
                "temporary logind failure".to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    #[zbus(name = "IsHomeReady")]
    fn is_home_ready(&self) -> bool {
        let state = self
            .0
            .lock()
            .unwrap_or_else(|e| panic!("fixture lock: {e}"));
        !state.rejected
            && state
                .progress
                .as_ref()
                .is_none_or(|p| p.phase == HomeResetPhase::Verified)
    }

    #[zbus(name = "QueryHomeReset")]
    fn query_home_reset(&self) -> Option<HomeResetProgress> {
        self.0
            .lock()
            .unwrap_or_else(|e| panic!("fixture lock: {e}"))
            .progress
            .clone()
    }

    #[zbus(name = "PrepareHomeReset")]
    fn prepare_home_reset(
        &mut self,
        epoch: u64,
        waiting: GraphicalSession,
    ) -> Result<(), ResourceControlError> {
        assert_eq!(self.query_managed_sessions().waiting.session, Some(waiting));
        assert_eq!(
            self.query_managed_sessions().foreground,
            SessionForeground::Waiting
        );
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|e| panic!("fixture lock: {e}"));
        state.assert_blocked();
        if state.rejected {
            return Err(ResourceControlError::Rejected("unmanaged mount".to_owned()));
        }
        state.progress = Some(HomeResetProgress {
            reset_epoch: epoch,
            phase: HomeResetPhase::Prepared,
        });
        Ok(())
    }

    #[zbus(name = "ApplyHomeReset")]
    fn apply_home_reset(&mut self, epoch: u64) -> Result<(), ResourceControlError> {
        self.finish_home(epoch)
    }

    #[zbus(name = "RecoverHomeReset")]
    fn recover_home_reset(&mut self, epoch: u64) -> Result<(), ResourceControlError> {
        self.finish_home(epoch)
    }

    #[zbus(name = "VerifyHomeReset")]
    fn verify_home_reset(&mut self, epoch: u64) -> HomeResetProgress {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|e| panic!("fixture lock: {e}"));
        let phase = if state.home_calls.len() > state.home_failures && !state.rejected {
            HomeResetPhase::Verified
        } else {
            HomeResetPhase::RecoveryRequired
        };
        let progress = HomeResetProgress {
            reset_epoch: epoch,
            phase,
        };
        state.progress = Some(progress.clone());
        progress
    }
}

impl FaultyHelper {
    fn finish_home(&self, epoch: u64) -> Result<(), ResourceControlError> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|e| panic!("fixture lock: {e}"));
        state.assert_blocked();
        state.home_calls.push(epoch);
        if state.rejected {
            Err(ResourceControlError::Rejected("unmanaged mount".to_owned()))
        } else if state.home_calls.len() <= state.home_failures {
            Err(ResourceControlError::Unavailable(
                "temporary mount failure".to_owned(),
            ))
        } else {
            if let Some(session) = state.session_after_home {
                state.session_state = Some(session);
            }
            Ok(())
        }
    }
}

impl HelperState {
    fn assert_blocked(&self) {
        if let Some(path) = &self.require_blocked {
            let mode = std::fs::read(path).ok().and_then(|encoded| {
                serde_json::from_slice::<caddy::CaddyModeArtifact>(&encoded).ok()
            });
            assert!(
                matches!(mode, Some(caddy::CaddyModeArtifact::Blocked { .. })),
                "Home/terminate effect started without confirmed BLOCKED"
            );
        }
    }
}

#[tokio::test]
async fn disconnected_recovery_finishes_only_captured_work()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture(HelperState {
        terminate_failures: 1,
        home_failures: 1,
        foreground: Some(SessionForeground::Waiting),
        ..HelperState::default()
    })
    .await?;
    fixture
        .helper
        .lock()
        .map_err(|_| "fixture lock")?
        .require_blocked = Some(fixture.directory.path().join("caddy-mode.json"));
    fixture.snapshots.deactivate().await?;
    let target = session::validate_target(SessionControlTarget {
        foreground_target: ForegroundTarget::Contest.into(),
        terminate_epoch: Some(7),
    })
    .ok_or("invalid target")?;
    let cancelled = CancellationToken::new();
    assert!(
        fixture
            .snapshots
            .session
            .reconcile(&target, Some(&waiting_session()), &cancelled)
            .await?
            .retry
    );
    assert!(
        fixture
            .snapshots
            .home
            .reconcile(Some(8), Some(&waiting_session()), &cancelled)
            .await?
            .retry
    );
    cancelled.cancel();

    let recovered = fixture
        .snapshots
        .recover_local()
        .await?
        .actual
        .ok_or("missing actual")?;
    assert_eq!(
        recovered.home.ok_or("missing Home")?.completed_reset_epoch,
        Some(8)
    );
    assert_eq!(
        recovered
            .session_control
            .ok_or("missing Session")?
            .completed_terminate_epoch,
        Some(7)
    );
    let state = fixture.helper.lock().map_err(|_| "fixture lock")?;
    assert_eq!(state.home_calls, [8, 8]);
    assert_eq!(state.termination_calls.len(), 2);
    assert_eq!(state.termination_calls[0], state.termination_calls[1]);
    assert_eq!(state.termination_calls[1].logind_session_id, "c2");
    state.assert_blocked();
    Ok(())
}

#[tokio::test]
async fn disconnected_home_recovery_returns_from_greeter_to_waiting()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture(HelperState {
        foreground: Some(SessionForeground::Greeter),
        session_after_home: Some(GraphicalSessionState::None),
        progress: Some(HomeResetProgress {
            reset_epoch: 8,
            phase: HomeResetPhase::Prepared,
        }),
        ..HelperState::default()
    })
    .await?;
    for _ in 0..2 {
        let actual = fixture
            .snapshots
            .recover_local()
            .await?
            .actual
            .ok_or("missing actual")?;
        assert_eq!(
            actual.home.ok_or("missing Home")?.completed_reset_epoch,
            Some(8)
        );
        let session = actual.session_control.ok_or("missing Session")?;
        assert_eq!(session.foreground, i32::from(WireForeground::Waiting));
        assert!(session.waiting_ready);
        assert!(!session.contest_ready);
    }
    let state = fixture.helper.lock().map_err(|_| "fixture lock")?;
    assert_eq!(state.home_calls, [8]);
    assert_eq!(
        state.activation_calls,
        [(
            natsume_local_control_api::SessionRole::Waiting,
            waiting_session()
        )]
    );
    assert!(state.preparation_calls.is_empty());
    assert!(state.termination_calls.is_empty());
    Ok(())
}

#[tokio::test]
async fn disconnected_home_recovery_preserves_other_foregrounds_and_pending_login()
-> Result<(), Box<dyn std::error::Error>> {
    for (session, foreground) in [
        (GraphicalSessionState::Running, SessionForeground::Contest),
        (GraphicalSessionState::None, SessionForeground::Other),
        (GraphicalSessionState::Starting, SessionForeground::Greeter),
        (GraphicalSessionState::Ambiguous, SessionForeground::Greeter),
    ] {
        let fixture = fixture(HelperState {
            foreground: Some(foreground),
            session_after_home: Some(session),
            progress: Some(HomeResetProgress {
                reset_epoch: 8,
                phase: HomeResetPhase::Prepared,
            }),
            ..HelperState::default()
        })
        .await?;
        fixture.snapshots.recover_local().await?;
        let state = fixture.helper.lock().map_err(|_| "fixture lock")?;
        assert_eq!(state.foreground, Some(foreground));
        assert!(state.activation_calls.is_empty());
        assert!(state.preparation_calls.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn cancelled_plan_finishes_owned_home_before_refusing_a_new_epoch()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture(HelperState {
        progress: Some(HomeResetProgress {
            reset_epoch: 8,
            phase: HomeResetPhase::Prepared,
        }),
        ..HelperState::default()
    })
    .await?;
    fixture.snapshots.deactivate().await?;
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        fixture
            .snapshots
            .home
            .reconcile(Some(9), Some(&waiting_session()), &cancelled)
            .await,
        Err(SnapshotError::Cancelled)
    ));
    assert_eq!(
        fixture
            .snapshots
            .home
            .observe()
            .await?
            .completed_reset_epoch,
        Some(8)
    );
    assert_eq!(
        fixture
            .helper
            .lock()
            .map_err(|_| "fixture lock")?
            .home_calls,
        [8]
    );
    let older = fixture
        .snapshots
        .home
        .reconcile(Some(7), Some(&waiting_session()), &cancelled)
        .await?
        .actual;
    assert_eq!(older.completed_reset_epoch, Some(8));
    let target = validate_server_snapshot(snapshot())?;
    assert!(!local_access_is_allowed(
        &target,
        &fixture.snapshots.session.observe().await?,
        &older
    ));
    Ok(())
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "keep service replacement and both durable recovery assertions together"
)]
async fn helper_owner_replacement_preserves_pending_session_and_home_work()
-> Result<(), Box<dyn std::error::Error>> {
    use natsume_local_control_api::{PRIVILEGED1_SERVICE, Privileged1Proxy};
    use tokio::time::{Duration, timeout};

    let directory = tempfile::tempdir()?;
    let (mut bus, address) = private_bus(&directory).await?;
    let connection = zbus::connection::Builder::address(address.as_str())?
        .method_timeout(Duration::from_secs(2))
        .build()
        .await?;
    let daemon_owner = connection.unique_name().cloned();
    let helper = Arc::new(Mutex::new(HelperState {
        terminate_failures: 1,
        home_failures: 1,
        foreground: Some(SessionForeground::Waiting),
        ..HelperState::default()
    }));
    let service = register_helper(&address, Arc::clone(&helper)).await?;
    let old_owner = service.unique_name().cloned();
    let binding_input = Arc::new(binding::tests::input_provider(&directory));
    binding::tests::confirm_fixture_frame(
        &binding_input,
        connection
            .unique_name()
            .ok_or("missing daemon owner")?
            .as_str(),
        waiting_session(),
    );
    let session = session::tests::reconciler(&directory, connection.clone(), binding_input);
    let home = home::tests::reconciler(&directory, connection.clone());
    let target = session::validate_target(SessionControlTarget {
        foreground_target: ForegroundTarget::Contest.into(),
        terminate_epoch: Some(7),
    })
    .ok_or("invalid fixture target")?;
    let cancellation = CancellationToken::new();
    let waiting = waiting_session();

    let pending = session
        .reconcile(&target, Some(&waiting), &cancellation)
        .await?;
    assert!(pending.retry);
    assert_eq!(pending.actual.completed_terminate_epoch, None);
    let pending = home
        .reconcile(Some(8), Some(&waiting), &cancellation)
        .await?;
    assert!(pending.retry);
    assert_eq!(pending.actual.completed_reset_epoch, None);
    let session_path = directory.path().join("session-completion.json");
    let persisted_pending = std::fs::read(&session_path)?;

    service.close().await?;
    let bus_proxy = zbus::fdo::DBusProxy::new(&connection).await?;
    timeout(Duration::from_secs(2), async {
        while bus_proxy
            .name_has_owner(PRIVILEGED1_SERVICE.try_into()?)
            .await?
        {
            tokio::task::yield_now().await;
        }
        Ok::<_, zbus::Error>(())
    })
    .await??;
    let unavailable = session
        .reconcile(&target, Some(&waiting), &cancellation)
        .await?;
    assert!(unavailable.retry);
    assert_eq!(unavailable.actual.completed_terminate_epoch, None);
    let unavailable = home
        .reconcile(Some(8), Some(&waiting), &cancellation)
        .await?;
    assert!(unavailable.retry);
    assert_eq!(unavailable.actual.completed_reset_epoch, None);
    assert_eq!(
        session.observe().await?.session_state,
        i32::from(SessionState::Terminating)
    );
    assert_eq!(
        home.observe().await?.state,
        i32::from(HomeState::RecoveryRequired)
    );
    assert_eq!(std::fs::read(&session_path)?, persisted_pending);
    assert!(!directory.path().join("home-completion.json").exists());

    // A new bus owner reads the same Helper-owned durable progress; it may
    // observe a new graphical session, but the pending operation stays on c2.
    let replacement = register_helper(&address, Arc::clone(&helper)).await?;
    assert_ne!(replacement.unique_name(), old_owner.as_ref());
    assert_eq!(connection.unique_name(), daemon_owner.as_ref());
    let fresh = Privileged1Proxy::new(&connection)
        .await?
        .query_managed_sessions()
        .await?;
    assert_eq!(
        fresh
            .contest
            .session
            .ok_or("missing replacement session")?
            .logind_session_id,
        "c3"
    );
    let completed = session
        .reconcile(&target, Some(&waiting), &cancellation)
        .await?;
    assert!(!completed.retry);
    assert_eq!(completed.actual.completed_terminate_epoch, Some(7));
    let completed = home
        .reconcile(Some(8), Some(&waiting), &cancellation)
        .await?;
    assert!(!completed.retry);
    assert_eq!(completed.actual.completed_reset_epoch, Some(8));
    assert_eq!(session.observe().await?.completed_terminate_epoch, Some(7));
    assert_eq!(home.observe().await?.completed_reset_epoch, Some(8));
    {
        let state = helper
            .lock()
            .unwrap_or_else(|error| panic!("fixture lock: {error}"));
        assert_eq!(state.termination_calls.len(), 2);
        assert_eq!(state.termination_calls[0], state.termination_calls[1]);
        assert_eq!(state.termination_calls[1].logind_session_id, "c2");
        assert_eq!(state.home_calls, [8, 8]);
    }
    replacement.close().await?;
    bus.kill().await?;
    Ok(())
}

async fn private_bus(
    directory: &tempfile::TempDir,
) -> Result<(tokio::process::Child, String), Box<dyn std::error::Error>> {
    use std::process::Stdio;
    use tokio::{
        io::{AsyncBufReadExt as _, BufReader},
        time::{Duration, timeout},
    };

    let mut bus = tokio::process::Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
        .arg(format!(
            "--address=unix:path={}",
            directory.path().join("bus.sock").display()
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let mut output = BufReader::new(bus.stdout.take().ok_or("missing bus stdout")?);
    let mut address = String::new();
    timeout(Duration::from_secs(5), output.read_line(&mut address)).await??;
    if address.trim().is_empty() {
        return Err("private bus did not publish its address".into());
    }
    Ok((bus, address.trim().to_owned()))
}

async fn register_helper(
    address: &str,
    helper: Arc<Mutex<HelperState>>,
) -> Result<zbus::Connection, zbus::Error> {
    zbus::connection::Builder::address(address)?
        .name(natsume_local_control_api::PRIVILEGED1_SERVICE)?
        .serve_at(PRIVILEGED1_PATH, FaultyHelper(helper))?
        .build()
        .await
}

pub(crate) struct Fixture {
    pub(crate) snapshots: Arc<SnapshotReconciler>,
    pub(crate) helper: Arc<Mutex<HelperState>>,
    pub(crate) directory: tempfile::TempDir,
    service: zbus::Connection,
    caddy_task: tokio::task::JoinHandle<()>,
    gateway_task: Option<tokio::task::JoinHandle<()>>,
    agent_task: tokio::task::JoinHandle<()>,
    _bus: tokio::process::Child,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.caddy_task.abort();
        self.agent_task.abort();
        if let Some(task) = &self.gateway_task {
            task.abort();
        }
    }
}

impl Fixture {
    pub(crate) async fn pause_agent_frames(&self) {
        self.agent_task.abort();
        while !self.agent_task.is_finished() {
            tokio::task::yield_now().await;
        }
        binding::tests::withdraw_fixture_frame(&self.snapshots.binding_input);
    }

    pub(crate) fn confirm_agent_frame(&self) -> Result<(), Box<dyn std::error::Error>> {
        let sender = self.service.unique_name().ok_or("missing fixture owner")?;
        binding::tests::confirm_fixture_frame(
            &self.snapshots.binding_input,
            sender.as_str(),
            waiting_session(),
        );
        Ok(())
    }

    pub(crate) async fn wait_for_plan(&self) -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !binding::tests::has_fixture_plan(&self.snapshots.binding_input) {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await?;
        Ok(())
    }
}

pub(crate) async fn fixture(state: HelperState) -> Result<Fixture, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let helper = Arc::new(Mutex::new(state));
    let (bus, address) = private_bus(&directory).await?;
    let server = register_helper(&address, Arc::clone(&helper)).await?;
    let connection = zbus::connection::Builder::address(address.as_str())?
        .method_timeout(std::time::Duration::from_secs(2))
        .build()
        .await?;
    let (caddy, caddy_task) = caddy::tests::fixture(&directory)?;
    let binding_input = Arc::new(binding::tests::input_provider(&directory));
    let sender = server
        .unique_name()
        .ok_or("missing fixture bus owner")?
        .to_string();
    binding::tests::confirm_fixture_frame(&binding_input, &sender, waiting_session());
    let agent = Arc::clone(&binding_input);
    let agent_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            binding::tests::confirm_fixture_frame(&agent, &sender, waiting_session());
        }
    });
    let images = directory.path().join("logos");
    std::fs::create_dir(&images)?;
    let presentation = Arc::new(presentation::Presentation::new(
        directory.path().join("waiting.json"),
        images,
        Arc::clone(&binding_input),
    ));
    presentation.configure("fixture-scope".to_owned())?;
    let snapshots = Arc::new(SnapshotReconciler {
        presentation,
        gateway: gateway::tests::reconciler(&directory),
        binding_input: Arc::clone(&binding_input),
        binding: binding::tests::reconciler(&directory),
        runtime: runtime::tests::reconciler(&directory),
        session: session::tests::reconciler(&directory, connection.clone(), binding_input),
        home: home::tests::reconciler(&directory, connection.clone()),
        power: power::reconciler(&directory, connection),
        caddy,
    });
    Ok(Fixture {
        snapshots,
        helper,
        directory,
        service: server,
        caddy_task,
        gateway_task: None,
        agent_task,
        _bus: bus,
    })
}

fn waiting_session() -> GraphicalSession {
    GraphicalSession {
        logind_session_id: "w1".to_owned(),
        boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
    }
}

async fn ready_fixture() -> Result<(Fixture, ValidatedSnapshot), Box<dyn std::error::Error>> {
    use natsume_device_protocol::generated::{GatewayCertificateGrant, GatewayState};
    use rustls_pki_types::pem::PemObject as _;
    let mut fixture = fixture(HelperState::default()).await?;
    let mut target = validate_server_snapshot(snapshot())?;
    let resources = Arc::get_mut(&mut fixture.snapshots).ok_or("fixture already shared")?;
    resources.gateway.current_input(&target.gateway_target)?;
    let key_pem = zeroize::Zeroizing::new(std::fs::read(
        fixture
            .directory
            .path()
            .join("gateway")
            .join(target.gateway_target.credential_id.to_string())
            .join("key.pem"),
    )?);
    let key = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(
        &rustls_pki_types::PrivatePkcs8KeyDer::from_pem_slice(&key_pem)?,
        &rcgen::PKCS_ECDSA_P256_SHA256,
    )?;
    let (certificate, task) =
        caddy::tests::serve_gateway(&mut resources.caddy, &fixture.directory, &key).await?;
    fixture.gateway_task = Some(task);
    target.gateway_target = gateway::validate_target(GatewayTarget {
        credential_id: target.gateway_target.credential_id.to_string(),
        certificate: Some(GatewayCertificateGrant {
            gateway_leaf_der: certificate.der().to_vec(),
        }),
    })
    .ok_or("fixture certificate")?;
    target.binding_target.bound = Some(binding::ValidatedBoundTarget {
        context: binding::ValidatedBindingContext {
            binding_id: Uuid::now_v7().to_string(),
            account_id: Uuid::now_v7().to_string(),
            seat_code: "A-01".to_owned(),
            domjudge_username: "team-alpha".to_owned(),
            credential_revision: 1,
        },
        password: zeroize::Zeroizing::new("password-canary".to_owned()),
    });
    target.binding_intent = None;
    let fence = CancellationToken::new();
    resources.begin_plan(&fence)?;
    let actual = resources
        .reconcile(&target, fence)
        .await?
        .actual
        .actual
        .ok_or("missing actual")?;
    assert_eq!(
        actual.gateway.ok_or("missing gateway")?.state,
        i32::from(GatewayState::Ready)
    );
    fixture
        .helper
        .lock()
        .map_err(|_| "fixture lock")?
        .require_blocked = Some(fixture.directory.path().join("caddy-mode.json"));
    Ok((fixture, target))
}

pub(crate) fn snapshot() -> ServerStateSnapshot {
    let credential_id = Uuid::now_v7().hyphenated().to_string();
    ServerStateSnapshot {
        intent: Some(ServerIntentState {
            gateway_credential: Some(GatewayCredentialIntent {
                credential_id: credential_id.clone(),
            }),
            binding: Some(BindingNegotiationIntent {
                negotiation_id: Uuid::now_v7().hyphenated().to_string(),
                evaluation: None,
            }),
        }),
        target: Some(ConcreteTargetState {
            gateway: Some(GatewayTarget {
                credential_id,
                certificate: None,
            }),
            binding_access: Some(BindingAccessTarget { bound: None }),
            runtime_config: Some(RuntimeConfigTarget {
                domjudge_origin: "https://judge.example".to_owned(),
            }),
            session_control: Some(SessionControlTarget {
                foreground_target: ForegroundTarget::Contest.into(),
                terminate_epoch: None,
            }),
            home: Some(HomeTarget { reset_epoch: None }),
            power: Some(natsume_device_protocol::generated::PowerControlTarget {
                shutdown_epoch: None,
                expires_at_unix_ms: None,
            }),
        }),
    }
}

#[tokio::test]
async fn observation_revokes_binding_and_loaded_access_without_a_new_target()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture(HelperState::default()).await?;
    let wire = snapshot();
    let negotiation = wire
        .intent
        .as_ref()
        .and_then(|intent| intent.binding.as_ref())
        .ok_or("missing intent")?
        .negotiation_id
        .clone();
    let target = validate_server_snapshot(wire)?;
    let fence = CancellationToken::new();
    fixture.snapshots.begin_plan(&fence)?;
    fixture.snapshots.reconcile(&target, fence).await?;
    // The Caddy fixture exercises real mode/config persistence and admin I/O.
    // Seed a previously loaded READY mode without requiring a public TLS port.
    std::fs::write(
        fixture.directory.path().join("caddy-mode.json"),
        serde_json::to_vec(&caddy::CaddyModeArtifact::Ready {
            format_version: 1,
            credential_id: target.gateway_target.credential_id.to_string(),
            domjudge_origin: target.runtime_origin.clone(),
            binding: binding::ValidatedBindingContext {
                binding_id: Uuid::now_v7().to_string(),
                account_id: Uuid::now_v7().to_string(),
                seat_code: "A-01".to_owned(),
                domjudge_username: "team-alpha".to_owned(),
                credential_revision: 1,
            },
        })?,
    )?;
    fixture
        .helper
        .lock()
        .map_err(|_| "fixture lock")?
        .session_state = Some(GraphicalSessionState::Ambiguous);
    fixture.snapshots.observe(Some(&target)).await?;
    assert!(matches!(
        fixture
            .snapshots
            .binding_input
            .submit(&negotiation, 1, "A-01"),
        Err(SnapshotError::StaleLocalInput)
    ));
    assert!(matches!(
        fixture.snapshots.caddy.observe().await.mode,
        Some(caddy::CaddyModeArtifact::Blocked { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn observation_cannot_reuse_a_binding_frame_after_withdrawing_input()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture(HelperState::default()).await?;
    let target = validate_server_snapshot(snapshot())?;
    let fence = CancellationToken::new();
    fixture.snapshots.begin_plan(&fence)?;
    fixture.snapshots.reconcile(&target, fence).await?;
    fixture.agent_task.abort();
    while !fixture.agent_task.is_finished() {
        tokio::task::yield_now().await;
    }
    let caller = fixture.service.unique_name().ok_or("missing owner")?;
    binding::tests::confirm_fixture_frame(
        &fixture.snapshots.binding_input,
        caller.as_str(),
        waiting_session(),
    );
    assert!(fixture.snapshots.session.observe().await?.waiting_ready);
    fixture
        .helper
        .lock()
        .map_err(|_| "fixture lock")?
        .foreground = Some(SessionForeground::Contest);

    let actual = fixture
        .snapshots
        .observe(Some(&target))
        .await?
        .actual
        .and_then(|actual| actual.session_control)
        .ok_or("missing session actual")?;
    assert!(!actual.waiting_ready);
    // No replacement frame has been supplied. A second observation cannot
    // restore the withdrawn readiness or input just from a healthy desktop.
    assert!(!fixture.snapshots.session.observe().await?.waiting_ready);
    assert!(!fixture.snapshots.binding_input.revoke_eligibility()?);
    Ok(())
}

#[test]
fn complete_server_snapshot_is_validated_before_reconciliation() {
    assert!(validate_server_snapshot(snapshot()).is_ok());

    let mut invalid = snapshot();
    if let Some(gateway) = invalid
        .target
        .as_mut()
        .and_then(|target| target.gateway.as_mut())
    {
        gateway.credential_id = Uuid::now_v7().hyphenated().to_string();
    }
    assert!(matches!(
        validate_server_snapshot(invalid),
        Err(SnapshotError::InvalidServerSnapshot)
    ));
}

#[test]
fn unbound_target_requires_a_negotiation_intent() {
    let mut invalid = snapshot();
    if let Some(intent) = invalid.intent.as_mut() {
        intent.binding = None;
    }

    assert!(matches!(
        validate_server_snapshot(invalid),
        Err(SnapshotError::InvalidServerSnapshot)
    ));
}

#[test]
fn caddy_ready_requires_the_exact_applied_runtime_origin() {
    let target = RuntimeConfigTarget {
        domjudge_origin: "https://judge.example".to_owned(),
    };
    let failed = RuntimeConfigActualState {
        state: RuntimeConfigState::Failed.into(),
        applied_domjudge_origin: Some("https://old.example".to_owned()),
    };
    let wrong = RuntimeConfigActualState {
        state: RuntimeConfigState::Applied.into(),
        applied_domjudge_origin: Some("https://other.example".to_owned()),
    };
    let exact = RuntimeConfigActualState {
        state: RuntimeConfigState::Applied.into(),
        applied_domjudge_origin: Some(target.domjudge_origin.clone()),
    };

    assert_eq!(
        applied_runtime_origin(&target.domjudge_origin, &failed),
        None
    );
    assert_eq!(
        applied_runtime_origin(&target.domjudge_origin, &wrong),
        None
    );
    assert_eq!(
        applied_runtime_origin(&target.domjudge_origin, &exact),
        Some("https://judge.example")
    );
}

#[test]
fn local_access_requires_a_ready_contest_in_either_foreground() {
    let target = validate_server_snapshot(snapshot())
        .unwrap_or_else(|error| panic!("fixture target: {error}"));
    let home = HomeActualState {
        state: HomeState::Steady.into(),
        completed_reset_epoch: None,
    };
    for foreground in [
        natsume_device_protocol::generated::SessionForeground::Contest,
        natsume_device_protocol::generated::SessionForeground::Waiting,
    ] {
        assert!(local_access_is_allowed(
            &target,
            &SessionControlActualState {
                contest_ready: true,
                waiting_ready: false,
                foreground: foreground.into(),
                session_state: SessionState::Running.into(),
                completed_terminate_epoch: None,
            },
            &home,
        ));
    }
    assert!(!local_access_is_allowed(
        &target,
        &SessionControlActualState {
            session_state: SessionState::Running.into(),
            ..SessionControlActualState::default()
        },
        &home
    ));
    for state in [
        SessionState::Unspecified,
        SessionState::None,
        SessionState::Starting,
        SessionState::Terminating,
        SessionState::Ambiguous,
        SessionState::Error,
    ] {
        assert!(!local_access_is_allowed(
            &target,
            &SessionControlActualState {
                contest_ready: true,
                waiting_ready: false,
                foreground: natsume_device_protocol::generated::SessionForeground::Contest.into(),
                session_state: state.into(),
                completed_terminate_epoch: None,
            },
            &home,
        ));
    }
}

#[test]
fn local_access_requires_steady_home_and_exact_completed_epochs() -> Result<(), SnapshotError> {
    let mut target = validate_server_snapshot(snapshot())?;
    target.home_epoch = Some(7);
    target.session_target = session::validate_target(SessionControlTarget {
        foreground_target: ForegroundTarget::Contest.into(),
        terminate_epoch: Some(8),
    })
    .ok_or(SnapshotError::InvalidServerSnapshot)?;
    for home_epoch in [None, Some(6), Some(7), Some(9)] {
        for session_epoch in [None, Some(7), Some(8), Some(9)] {
            for state in [
                HomeState::Unspecified,
                HomeState::Resetting,
                HomeState::RecoveryRequired,
                HomeState::Steady,
            ] {
                assert_eq!(
                    local_access_is_allowed(
                        &target,
                        &SessionControlActualState {
                            contest_ready: true,
                            waiting_ready: false,
                            foreground:
                                natsume_device_protocol::generated::SessionForeground::Contest
                                    .into(),
                            session_state: SessionState::Running.into(),
                            completed_terminate_epoch: session_epoch,
                        },
                        &HomeActualState {
                            state: state.into(),
                            completed_reset_epoch: home_epoch
                        }
                    ),
                    home_epoch == Some(7) && session_epoch == Some(8) && state == HomeState::Steady
                );
            }
        }
    }
    assert!(!local_access_is_allowed(
        &target,
        &SessionControlActualState {
            contest_ready: true,
            waiting_ready: false,
            foreground: natsume_device_protocol::generated::SessionForeground::Contest.into(),
            session_state: 999,
            completed_terminate_epoch: Some(8)
        },
        &HomeActualState {
            state: HomeState::Steady.into(),
            completed_reset_epoch: Some(7)
        }
    ));
    Ok(())
}

#[tokio::test]
async fn loaded_ready_is_blocked_on_local_failure_and_restored_by_current_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let (fixture, target) = ready_fixture().await?;
    let binding = std::fs::read(fixture.directory.path().join("binding-assignment.json"))?;
    for state in [
        GraphicalSessionState::None,
        GraphicalSessionState::Ambiguous,
    ] {
        fixture
            .helper
            .lock()
            .map_err(|_| "fixture lock")?
            .session_state = Some(state);
        fixture.snapshots.observe(Some(&target)).await?;
        assert!(matches!(
            fixture.snapshots.caddy.observe().await.mode,
            Some(caddy::CaddyModeArtifact::Blocked { .. })
        ));
        fixture
            .helper
            .lock()
            .map_err(|_| "fixture lock")?
            .session_state = Some(GraphicalSessionState::Running);
        // Observation has no authority to grant access again.
        fixture.snapshots.observe(Some(&target)).await?;
        assert!(matches!(
            fixture.snapshots.caddy.observe().await.mode,
            Some(caddy::CaddyModeArtifact::Blocked { .. })
        ));
        let fence = CancellationToken::new();
        fixture.snapshots.begin_plan(&fence)?;
        fixture.snapshots.reconcile(&target, fence).await?;
        assert!(matches!(
            fixture.snapshots.caddy.observe().await.mode,
            Some(caddy::CaddyModeArtifact::Ready { .. })
        ));
        assert_eq!(
            std::fs::read(fixture.directory.path().join("binding-assignment.json"))?,
            binding
        );
    }
    Ok(())
}

#[tokio::test]
async fn pending_epochs_block_ready_before_helper_effects_and_wait_for_completion()
-> Result<(), Box<dyn std::error::Error>> {
    let (fixture, mut target) = ready_fixture().await?;
    target.home_epoch = Some(7);
    target.session_target = session::validate_target(SessionControlTarget {
        foreground_target: ForegroundTarget::Contest.into(),
        terminate_epoch: Some(8),
    })
    .ok_or(SnapshotError::InvalidServerSnapshot)?;
    {
        let mut state = fixture.helper.lock().map_err(|_| "fixture lock")?;
        state.terminate_failures = 1;
        state.home_failures = 1;
    }
    for complete in [false, true] {
        let fence = CancellationToken::new();
        fixture.snapshots.begin_plan(&fence)?;
        let outcome = fixture.snapshots.reconcile(&target, fence).await?;
        let actual = outcome.actual.actual.ok_or("missing actual")?;
        assert_eq!(outcome.retry, !complete);
        assert_eq!(
            actual.home.ok_or("missing Home")?.completed_reset_epoch,
            complete.then_some(7)
        );
        assert_eq!(
            actual
                .session_control
                .ok_or("missing Session")?
                .completed_terminate_epoch,
            complete.then_some(8)
        );
        assert_eq!(
            matches!(
                fixture.snapshots.caddy.observe().await.mode,
                Some(caddy::CaddyModeArtifact::Ready { .. })
            ),
            complete
        );
    }
    Ok(())
}

#[tokio::test]
async fn failed_contest_preparation_retries_without_repeating_maintenance()
-> Result<(), Box<dyn std::error::Error>> {
    let (fixture, target) = ready_fixture().await?;
    let binding = std::fs::read(fixture.directory.path().join("binding-assignment.json"))?;
    {
        let mut state = fixture.helper.lock().map_err(|_| "fixture lock")?;
        state.session_state = Some(GraphicalSessionState::Error);
        state.preparation_calls.clear();
    }
    let fence = CancellationToken::new();
    fixture.snapshots.begin_plan(&fence)?;
    let outcome = fixture.snapshots.reconcile(&target, fence).await?;
    assert!(outcome.retry);
    assert_eq!(
        outcome
            .actual
            .actual
            .ok_or("missing actual")?
            .session_control
            .ok_or("missing session")?
            .session_state,
        i32::from(SessionState::Error)
    );
    {
        let state = fixture.helper.lock().map_err(|_| "fixture lock")?;
        assert_eq!(
            state.preparation_calls,
            [natsume_local_control_api::SessionRole::Contest]
        );
        assert!(state.home_calls.is_empty());
        assert!(state.termination_calls.is_empty());
        assert!(state.activation_calls.is_empty());
        state.assert_blocked();
    }
    fixture
        .helper
        .lock()
        .map_err(|_| "fixture lock")?
        .session_state = Some(GraphicalSessionState::Running);
    let fence = CancellationToken::new();
    fixture.snapshots.begin_plan(&fence)?;
    fixture.snapshots.reconcile(&target, fence).await?;
    assert!(matches!(
        fixture.snapshots.caddy.observe().await.mode,
        Some(caddy::CaddyModeArtifact::Ready { .. })
    ));
    assert_eq!(
        fixture
            .helper
            .lock()
            .map_err(|_| "fixture lock")?
            .preparation_calls,
        [natsume_local_control_api::SessionRole::Contest]
    );
    assert_eq!(
        std::fs::read(fixture.directory.path().join("binding-assignment.json"))?,
        binding
    );
    Ok(())
}

#[tokio::test]
async fn home_effect_cannot_reopen_access_from_the_pre_reset_session_observation()
-> Result<(), Box<dyn std::error::Error>> {
    let (fixture, mut target) = ready_fixture().await?;
    target.home_epoch = Some(7);
    fixture
        .helper
        .lock()
        .map_err(|_| "fixture lock")?
        .session_after_home = Some(GraphicalSessionState::None);
    let fence = CancellationToken::new();
    fixture.snapshots.begin_plan(&fence)?;
    let actual = fixture
        .snapshots
        .reconcile(&target, fence)
        .await?
        .actual
        .actual
        .ok_or("missing actual")?;
    assert_eq!(
        actual.home.ok_or("missing Home")?.state,
        i32::from(HomeState::Steady)
    );
    assert_eq!(
        actual
            .session_control
            .ok_or("missing Session")?
            .session_state,
        i32::from(SessionState::None)
    );
    assert!(matches!(
        fixture.snapshots.caddy.observe().await.mode,
        Some(caddy::CaddyModeArtifact::Blocked { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn new_maintenance_waits_for_a_live_waiting_frame_before_capturing_contest()
-> Result<(), Box<dyn std::error::Error>> {
    let (fixture, mut target) = ready_fixture().await?;
    fixture.agent_task.abort();
    binding::tests::clear_fixture_agent(&fixture.snapshots.binding_input);
    target.home_epoch = Some(7);
    target.session_target = session::validate_target(SessionControlTarget {
        foreground_target: ForegroundTarget::Contest.into(),
        terminate_epoch: Some(8),
    })
    .ok_or("invalid target")?;
    let fence = CancellationToken::new();
    fixture.snapshots.begin_plan(&fence)?;
    let outcome = fixture.snapshots.reconcile(&target, fence).await?;
    assert!(outcome.retry);
    let state = fixture.helper.lock().map_err(|_| "fixture lock")?;
    state.assert_blocked();
    assert!(state.termination_calls.is_empty());
    assert!(state.home_calls.is_empty());
    assert!(state.activation_calls.is_empty());
    assert!(state.preparation_calls.is_empty());
    assert!(state.progress.is_none());
    assert!(
        !fixture
            .directory
            .path()
            .join("session-completion.json")
            .exists()
    );
    Ok(())
}

#[tokio::test]
async fn failed_blocking_aborts_before_any_home_or_terminate_effect()
-> Result<(), Box<dyn std::error::Error>> {
    let (fixture, mut target) = ready_fixture().await?;
    target.home_epoch = Some(7);
    target.session_target = session::validate_target(SessionControlTarget {
        foreground_target: ForegroundTarget::Contest.into(),
        terminate_epoch: Some(8),
    })
    .ok_or(SnapshotError::InvalidServerSnapshot)?;
    std::fs::write(
        fixture.directory.path().join("caddy-fixture"),
        "#!/bin/sh\nexit 1\n",
    )?;
    let fence = CancellationToken::new();
    fixture.snapshots.begin_plan(&fence)?;
    assert!(matches!(
        fixture.snapshots.reconcile(&target, fence).await,
        Err(SnapshotError::Caddy)
    ));
    {
        let mut state = fixture.helper.lock().map_err(|_| "fixture lock")?;
        assert!(
            state.termination_calls.is_empty()
                && state.home_calls.is_empty()
                && state.progress.is_none()
        );
        state.session_state = Some(GraphicalSessionState::Ambiguous);
    }
    assert!(matches!(
        fixture.snapshots.observe(Some(&target)).await,
        Err(SnapshotError::Caddy)
    ));
    Ok(())
}

#[tokio::test]
async fn binding_keeps_waiting_until_the_operator_selects_contest()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture(HelperState {
        foreground: Some(SessionForeground::Waiting),
        ..HelperState::default()
    })
    .await?;
    let cancellation = CancellationToken::new();
    for (foreground, bound, expected) in [
        (ForegroundTarget::Waiting, false, SessionForeground::Waiting),
        (ForegroundTarget::Waiting, true, SessionForeground::Waiting),
        (ForegroundTarget::Contest, true, SessionForeground::Contest),
    ] {
        let target = session::validate_target(SessionControlTarget {
            foreground_target: foreground.into(),
            terminate_epoch: None,
        })
        .ok_or("invalid target")?;
        let outcome = fixture
            .snapshots
            .session
            .present(&target, bound, true, &cancellation)
            .await?;
        assert!(!outcome.retry);
        let state = fixture.helper.lock().map_err(|_| "fixture lock")?;
        assert_eq!(state.foreground, Some(expected));
        assert_eq!(
            state.activation_calls.len(),
            usize::from(expected == SessionForeground::Contest)
        );
        assert!(state.termination_calls.is_empty());
        assert!(state.home_calls.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn foreground_and_repeated_targets_keep_ready_without_reloading_caddy()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::MetadataExt as _;
    let (fixture, mut target) = ready_fixture().await?;
    let mode = fixture.directory.path().join("caddy-mode.json");
    let inode = std::fs::metadata(&mode)?.ino();
    for foreground in [
        ForegroundTarget::Waiting,
        ForegroundTarget::Contest,
        ForegroundTarget::Contest,
    ] {
        target.session_target = session::validate_target(SessionControlTarget {
            foreground_target: foreground.into(),
            terminate_epoch: None,
        })
        .ok_or("fixture Session target")?;
        let fence = CancellationToken::new();
        fixture.snapshots.begin_plan(&fence)?;
        fixture.snapshots.reconcile(&target, fence).await?;
        assert!(matches!(
            fixture.snapshots.caddy.observe().await.mode,
            Some(caddy::CaddyModeArtifact::Ready { .. })
        ));
        assert_eq!(std::fs::metadata(&mode)?.ino(), inode);
        assert_eq!(
            fixture
                .helper
                .lock()
                .map_err(|_| "fixture lock")?
                .foreground,
            Some(if foreground == ForegroundTarget::Waiting {
                SessionForeground::Waiting
            } else {
                SessionForeground::Contest
            })
        );
    }
    let state = fixture.helper.lock().map_err(|_| "fixture lock")?;
    assert_eq!(state.activation_calls.len(), 2);
    assert_eq!(state.activation_calls[0].1, waiting_session());
    assert_eq!(state.activation_calls[1].1.logind_session_id, "c2");
    assert!(state.preparation_calls.is_empty());
    assert!(state.termination_calls.is_empty());
    assert!(state.home_calls.is_empty());
    Ok(())
}

#[tokio::test]
async fn observation_cannot_revoke_replacement_plan_ownership_or_reopen_old_access()
-> Result<(), Box<dyn std::error::Error>> {
    let (fixture, mut target) = ready_fixture().await?;
    let old = CancellationToken::new();
    fixture.snapshots.begin_plan(&old)?;
    let replacement = CancellationToken::new();
    old.cancel();
    fixture.snapshots.begin_plan(&replacement)?;
    fixture
        .helper
        .lock()
        .map_err(|_| "fixture lock")?
        .session_state = Some(GraphicalSessionState::Ambiguous);
    fixture.snapshots.observe(Some(&target)).await?;
    fixture
        .helper
        .lock()
        .map_err(|_| "fixture lock")?
        .session_state = Some(GraphicalSessionState::Running);
    assert!(matches!(
        fixture.snapshots.reconcile(&target, old).await,
        Err(SnapshotError::Cancelled)
    ));
    assert!(matches!(
        fixture.snapshots.caddy.observe().await.mode,
        Some(caddy::CaddyModeArtifact::Blocked { .. })
    ));
    target.home_epoch = Some(7);
    fixture.snapshots.reconcile(&target, replacement).await?;
    assert!(matches!(
        fixture.snapshots.caddy.observe().await.mode,
        Some(caddy::CaddyModeArtifact::Ready { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn observation_blocks_a_previously_verified_home_that_needs_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    let (fixture, mut target) = ready_fixture().await?;
    target.home_epoch = Some(7);
    let fence = CancellationToken::new();
    fixture.snapshots.begin_plan(&fence)?;
    fixture.snapshots.reconcile(&target, fence).await?;
    fixture
        .helper
        .lock()
        .map_err(|_| "fixture lock")?
        .home_failures = usize::MAX;
    let actual = fixture
        .snapshots
        .observe(Some(&target))
        .await?
        .actual
        .ok_or("missing actual")?;
    assert_eq!(
        actual.home.ok_or("missing Home")?.state,
        i32::from(HomeState::RecoveryRequired)
    );
    assert!(matches!(
        fixture.snapshots.caddy.observe().await.mode,
        Some(caddy::CaddyModeArtifact::Blocked { .. })
    ));
    fixture
        .helper
        .lock()
        .map_err(|_| "fixture lock")?
        .home_failures = 0;
    let fence = CancellationToken::new();
    fixture.snapshots.begin_plan(&fence)?;
    fixture.snapshots.reconcile(&target, fence).await?;
    assert!(matches!(
        fixture.snapshots.caddy.observe().await.mode,
        Some(caddy::CaddyModeArtifact::Ready { .. })
    ));
    Ok(())
}

#[test]
fn cancelled_plan_is_rejected_before_an_effect() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    assert!(matches!(
        check_cancellation(&cancellation),
        Err(SnapshotError::Cancelled)
    ));
}

#[test]
fn bound_display_is_required_but_does_not_change_any_resource_target() {
    use natsume_device_protocol::generated::{
        BindingContext, BoundTarget, SecretBytes, TeamPresentation,
    };
    let mut wire = snapshot();
    wire.intent
        .as_mut()
        .unwrap_or_else(|| panic!("intent"))
        .binding = None;
    let bound = BoundTarget {
        context: Some(BindingContext {
            binding_id: Uuid::now_v7().to_string(),
            account_id: Uuid::now_v7().to_string(),
            seat_code: "A-01".into(),
            domjudge_username: "team-alpha".into(),
            credential_revision: 1,
        }),
        password: Some(SecretBytes {
            value: b"fixture-password".to_vec(),
        }),
        presentation: Some(TeamPresentation {
            team_name_zh: "队伍".into(),
            team_name_en: "Team".into(),
            school_name_zh: "示例大学".into(),
            school_name_en: String::new(),
            organization_id: "INST-001".into(),
        }),
    };
    wire.target
        .as_mut()
        .unwrap_or_else(|| panic!("target"))
        .binding_access =
        Some(natsume_device_protocol::generated::BindingAccessTarget { bound: Some(bound) });
    let original =
        validate_server_snapshot(wire.clone()).unwrap_or_else(|e| panic!("snapshot: {e}"));
    let mut renamed = wire.clone();
    let presentation = renamed
        .target
        .as_mut()
        .and_then(|t| t.binding_access.as_mut())
        .and_then(|t| t.bound.as_mut())
        .and_then(|t| t.presentation.as_mut())
        .unwrap_or_else(|| panic!("presentation"));
    presentation.team_name_zh = "新队名".into();
    presentation.organization_id = "INST-002".into();
    let updated = validate_server_snapshot(renamed).unwrap_or_else(|e| panic!("snapshot: {e}"));
    assert!(
        original == updated,
        "Display updates must not restart a resource plan"
    );
    wire.target
        .as_mut()
        .and_then(|t| t.binding_access.as_mut())
        .and_then(|t| t.bound.as_mut())
        .unwrap_or_else(|| panic!("bound"))
        .presentation = None;
    assert!(matches!(
        validate_server_snapshot(wire),
        Err(SnapshotError::InvalidServerSnapshot)
    ));
}

#[tokio::test]
async fn old_local_agent_contract_is_rejected_before_registration()
-> Result<(), Box<dyn std::error::Error>> {
    use natsume_local_control_api::{DEVICE1_PATH, DEVICE1_SERVICE, Device1Proxy};
    let fixture = fixture(HelperState::default()).await?;
    binding::DeviceService::start(
        &fixture.service,
        Arc::clone(&fixture.snapshots.binding_input),
    )
    .await?;
    let proxy = Device1Proxy::new(&fixture.service).await?;
    let error = proxy
        .register_session_agent(2, &waiting_session())
        .await
        .err()
        .ok_or("old version was accepted")?;
    assert!(
        error
            .to_string()
            .contains("incompatible Session Agent protocol")
    );
    let raw = zbus::Proxy::new(
        &fixture.service,
        DEVICE1_SERVICE,
        DEVICE1_PATH,
        DEVICE1_SERVICE,
    )
    .await?;
    let old: Result<
        (
            natsume_local_control_api::SessionAgentLease,
            natsume_local_control_api::SessionUiSnapshot,
        ),
        _,
    > = raw.call("RegisterSessionAgent", &waiting_session()).await;
    assert!(
        old.is_err(),
        "the old method signature cannot register an Agent"
    );
    Ok(())
}

#[tokio::test]
async fn display_updates_keep_ready_caddy_and_credentials_while_waiting_for_a_new_frame()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::MetadataExt as _;
    let (fixture, target) = ready_fixture().await?;
    fixture.agent_task.abort();
    let mode = fixture.directory.path().join("caddy-mode.json");
    let assignment = fixture.directory.path().join("binding-assignment.json");
    let inodes = (
        std::fs::metadata(&mode)?.ino(),
        std::fs::metadata(&assignment)?.ino(),
    );
    let context = &target.binding_target.bound.as_ref().ok_or("bound")?.context;
    for name in ["第一队名", "更新队名"] {
        fixture
            .snapshots
            .accept_presentation(Some(natsume_local_control_api::WaitingTeam {
                binding_id: context.binding_id.clone(),
                account_id: context.account_id.clone(),
                seat_code: context.seat_code.clone(),
                organization_id: "INST-001".into(),
                team_name_zh: name.into(),
                team_name_en: String::new(),
                school_name_zh: "大学".into(),
                school_name_en: String::new(),
            }))?;
        fixture.snapshots.observe(Some(&target)).await?;
        assert!(matches!(
            fixture.snapshots.caddy.observe().await.mode,
            Some(caddy::CaddyModeArtifact::Ready { .. })
        ));
        assert_eq!(
            (
                std::fs::metadata(&mode)?.ino(),
                std::fs::metadata(&assignment)?.ino()
            ),
            inodes
        );
    }
    let state = fixture.helper.lock().map_err(|_| "fixture lock")?;
    assert!(state.activation_calls.is_empty());
    assert!(state.preparation_calls.is_empty());
    assert!(state.termination_calls.is_empty());
    assert!(state.home_calls.is_empty());
    Ok(())
}
