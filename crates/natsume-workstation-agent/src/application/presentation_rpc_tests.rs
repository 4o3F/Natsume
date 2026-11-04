use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
};

use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

use natsume_workstation_ipc::{DEVICE1_PATH, SessionPresentation, WaitingTeam};
use tokio::net::UnixStream;

use super::*;

static SUBSCRIBER_LOCK: Mutex<()> = Mutex::new(());

struct SubscriberGuard {
    _lock: MutexGuard<'static, ()>,
    _dispatch: tracing::Dispatch,
}

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Text(String),
    Unsigned(u64),
}

#[derive(Clone, Debug)]
struct Record {
    fields: BTreeMap<String, Value>,
    level: tracing::Level,
}

impl Visit for Record {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record_str(field, &format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_owned(), Value::Text(value.to_owned()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_owned(), Value::Unsigned(value));
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<Record>>>);

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut record = Record {
            fields: BTreeMap::new(),
            level: *event.metadata().level(),
        };
        event.record(&mut record);
        if record.fields.get("message") == Some(&Value::Text("frame presented".into())) {
            self.0
                .lock()
                .unwrap_or_else(|error| panic!("capture: {error}"))
                .push(record);
        }
    }
}

impl Capture {
    fn events(&self) -> Vec<Record> {
        self.0
            .lock()
            .unwrap_or_else(|error| panic!("capture: {error}"))
            .clone()
    }
}

fn capture() -> (SubscriberGuard, Capture, tracing::Dispatch) {
    let guard = SubscriberGuard {
        _lock: SUBSCRIBER_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        _dispatch: tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()),
    };
    let capture = Capture::default();
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(capture.clone()));
    (guard, capture, dispatch)
}

fn assert_frame(record: &Record, revision: u64) {
    assert_eq!(record.level, tracing::Level::INFO);
    assert_eq!(
        record.fields.len(),
        5,
        "only fixed role, revision and timings may be logged"
    );
    assert_eq!(
        record.fields.get("message"),
        Some(&Value::Text("frame presented".into()))
    );
    assert_eq!(
        record.fields.get("role"),
        Some(&Value::Text("waiting".into()))
    );
    assert_eq!(
        record.fields.get("ui_revision"),
        Some(&Value::Unsigned(revision))
    );
    let Some(Value::Unsigned(since_lease)) = record.fields.get("since_lease_us") else {
        panic!("lease duration must be a typed unsigned field");
    };
    let Some(Value::Unsigned(since_revision)) = record.fields.get("since_revision_us") else {
        panic!("revision duration must be a typed unsigned field");
    };
    assert!(since_lease >= since_revision);
}

#[derive(Clone)]
struct DeviceFixture(Arc<Mutex<State>>);

impl DeviceFixture {
    fn state(&self) -> MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(|error| panic!("fixture: {error}"))
    }
}

struct State {
    lease: SessionAgentLease,
    snapshot: SessionUiSnapshot,
    live: bool,
    confirm_error: Option<zbus::fdo::Error>,
    get_error: Option<&'static str>,
    get_delay: Duration,
    gets: usize,
    confirms: usize,
    accepted: Option<SessionPresentation>,
}

#[zbus::interface(name = "org.natsume.Device1")]
impl DeviceFixture {
    fn confirm_session_presentation(
        &self,
        lease_id: &str,
        frame: SessionPresentation,
    ) -> zbus::fdo::Result<()> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|error| panic!("fixture: {error}"));
        state.confirms += 1;
        state.accepted = None;
        if let Some(error) = &state.confirm_error {
            return Err(error.clone());
        }
        if !state.live || lease_id != state.lease.lease_id {
            return Err(zbus::fdo::Error::Failed(
                "Session Agent lease is stale".into(),
            ));
        }
        if frame.session != state.lease.session {
            return Err(zbus::fdo::Error::Failed(
                "Session Agent caller was rejected".into(),
            ));
        }
        let dimensions_valid = if frame.first_frame_presented {
            frame.fullscreen_width > 0 && frame.fullscreen_height > 0
        } else {
            frame.fullscreen_width == 0 && frame.fullscreen_height == 0
        };
        if frame.agent_lease_id != lease_id
            || frame.ui_revision != state.snapshot.ui_revision
            || !dimensions_valid
        {
            return Err(zbus::fdo::Error::Failed(
                "Session presentation is incomplete or stale".into(),
            ));
        }
        state.accepted = frame.first_frame_presented.then_some(frame);
        Ok(())
    }

    async fn get_session_ui_snapshot(
        &self,
        lease_id: &str,
        session: GraphicalSession,
    ) -> zbus::fdo::Result<SessionUiSnapshot> {
        let (delay, result) = {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|error| panic!("fixture: {error}"));
            state.gets += 1;
            let result = if let Some(error) = state.get_error {
                Err(zbus::fdo::Error::Failed(error.into()))
            } else if !state.live || lease_id != state.lease.lease_id {
                Err(zbus::fdo::Error::Failed(
                    "Session Agent lease is stale".into(),
                ))
            } else if session != state.lease.session {
                Err(zbus::fdo::Error::Failed(
                    "Session Agent caller was rejected".into(),
                ))
            } else {
                Ok(state.snapshot.clone())
            };
            (state.get_delay, result)
        };
        tokio::time::sleep(delay).await;
        result
    }
}

fn fixture() -> (DeviceFixture, SessionAgentLease, SessionPresentation) {
    let session = GraphicalSession {
        logind_session_id: "waiting".into(),
        boot_id: "7138aed1-5921-4c4b-8e35-110806942def".into(),
    };
    let lease = SessionAgentLease {
        lease_id: "existing-lease".into(),
        session: session.clone(),
        expires_at_unix_ms: i64::MAX,
    };
    let frame = SessionPresentation {
        agent_lease_id: lease.lease_id.clone(),
        session: session.clone(),
        ui_revision: 10,
        first_frame_presented: true,
        fullscreen_width: 1920,
        fullscreen_height: 1080,
    };
    let snapshot = SessionUiSnapshot {
        agent_lease_id: lease.lease_id.clone(),
        ui_revision: 11,
        ..waiting_snapshot(&session)
    };
    (
        DeviceFixture(Arc::new(Mutex::new(State {
            lease: lease.clone(),
            snapshot,
            live: true,
            confirm_error: None,
            get_error: None,
            get_delay: Duration::ZERO,
            gets: 0,
            confirms: 0,
            accepted: None,
        }))),
        lease,
        frame,
    )
}

async fn peer(service: DeviceFixture) -> (zbus::Connection, zbus::Connection) {
    let (server, client) = UnixStream::pair().unwrap_or_else(|error| panic!("socket: {error}"));
    let server = zbus::connection::Builder::unix_stream(
        server
            .into_std()
            .unwrap_or_else(|error| panic!("server socket: {error}")),
    )
    .server(zbus::Guid::generate())
    .p2p()
    .serve_at(DEVICE1_PATH, service);
    let client = zbus::connection::Builder::unix_stream(
        client
            .into_std()
            .unwrap_or_else(|error| panic!("client socket: {error}")),
    )
    .p2p();
    let (server, client) = tokio::join!(server.build(), client.build());
    (
        server.unwrap_or_else(|error| panic!("server connection: {error}")),
        client.unwrap_or_else(|error| panic!("client connection: {error}")),
    )
}

async fn assert_confirmed(
    proxy: &Device1Proxy<'_>,
    lease: &SessionAgentLease,
    deadline: Instant,
    frame: &SessionPresentation,
    timing: &mut PresentationTiming,
) {
    assert!(matches!(
        confirm_presentation(proxy, lease, deadline, frame, timing).await,
        Ok(None)
    ));
}

#[tokio::test]
async fn superseded_frame_refreshes_same_lease_and_waits_for_actual_current_frame() {
    let (_guard, capture, dispatch) = capture();
    let _subscriber = tracing::dispatcher::set_default(&dispatch);
    let (service, lease, frame) = fixture();
    let mut timing = PresentationTiming::new(frame.ui_revision, Instant::now());
    let (_server, client) = peer(service.clone()).await;
    let proxy = Device1Proxy::new(&client)
        .await
        .unwrap_or_else(|error| panic!("proxy: {error}"));
    // Fetch revision 10, then advance the daemon before that frame is confirmed.
    service.state().snapshot.ui_revision = 10;
    let fetched = proxy
        .get_session_ui_snapshot(&lease.lease_id, &lease.session)
        .await
        .unwrap_or_else(|error| panic!("initial snapshot: {error}"));
    assert_eq!(fetched.ui_revision, frame.ui_revision);
    service.state().snapshot.ui_revision = 11;
    let deadline = Instant::now() + Duration::from_secs(2);
    let result = confirm_presentation(&proxy, &lease, deadline, &frame, &mut timing).await;
    let Ok(Some(snapshot)) = result else {
        panic!("superseded frame must refresh on the existing lease");
    };
    assert_eq!(snapshot.agent_lease_id, lease.lease_id);
    assert_eq!(snapshot.session, lease.session);
    assert_eq!(snapshot.ui_revision, 11);
    {
        let state = service.state();
        assert!(
            state.accepted.is_none(),
            "rejected frame must never become ready"
        );
        assert_eq!(
            state.confirms, 1,
            "refresh must not confirm a synthetic frame"
        );
    }
    assert!(
        capture.events().is_empty(),
        "superseded frames must be silent"
    );
    timing.queued(snapshot.ui_revision, Instant::now());
    let current_frame = SessionPresentation {
        ui_revision: snapshot.ui_revision,
        ..frame
    };
    assert_confirmed(&proxy, &lease, deadline, &current_frame, &mut timing).await;
    let state = service.state();
    assert_eq!(capture.events().len(), 1);
    assert_frame(&capture.events()[0], current_frame.ui_revision);
    assert_eq!(state.accepted.as_ref(), Some(&current_frame));
    assert_eq!(state.confirms, 2);
    assert_eq!(
        state.gets, 2,
        "current frame acceptance must not fetch again"
    );
}

#[tokio::test]
async fn superseded_binding_frame_preserves_authoritative_waiting_team_on_same_lease() {
    let (_guard, capture, dispatch) = capture();
    let _subscriber = tracing::dispatcher::set_default(&dispatch);
    for screen in [
        SessionScreenKind::BindingPrompt,
        SessionScreenKind::BindingPending,
    ] {
        let (service, lease, frame) = fixture();
        let mut timing = PresentationTiming::new(frame.ui_revision, Instant::now());
        let binding = SessionUiSnapshot {
            agent_lease_id: lease.lease_id.clone(),
            ui_revision: frame.ui_revision,
            screen,
            offline: false,
            negotiation_id: Some("completed-negotiation".into()),
            submission_epoch: Some(7),
            ..waiting_snapshot(&lease.session)
        };
        service.state().snapshot = binding.clone();
        let (_server, client) = peer(service.clone()).await;
        let proxy = Device1Proxy::new(&client)
            .await
            .unwrap_or_else(|error| panic!("proxy: {error}"));
        let fetched = proxy
            .get_session_ui_snapshot(&lease.lease_id, &lease.session)
            .await
            .unwrap_or_else(|error| panic!("binding snapshot: {error}"));
        assert_eq!(fetched, binding);

        let team = WaitingTeam {
            binding_id: "completed-binding".into(),
            account_id: "team-account".into(),
            seat_code: "B-12".into(),
            organization_id: "INST-002".into(),
            team_name_zh: "星辰队".into(),
            team_name_en: "Team Stars".into(),
            school_name_zh: "示例大学".into(),
            school_name_en: "Example University".into(),
        };
        let logo_path = "/var/lib/natsume-display/completed-team.png";
        let waiting = SessionUiSnapshot {
            agent_lease_id: lease.lease_id.clone(),
            ui_revision: frame.ui_revision + 1,
            team: Some(team.clone()),
            logo_path: Some(logo_path.into()),
            offline: false,
            ..waiting_snapshot(&lease.session)
        };
        // Binding completes after the Agent fetch, before its old frame RPC.
        service.state().snapshot = waiting.clone();
        let deadline = Instant::now() + Duration::from_secs(2);
        let Ok(Some(snapshot)) =
            confirm_presentation(&proxy, &lease, deadline, &frame, &mut timing).await
        else {
            panic!("superseded {screen:?} must recover the authoritative Waiting snapshot");
        };
        assert_eq!(
            snapshot, waiting,
            "all typed post-binding data must survive recovery"
        );
        assert_eq!(snapshot.team.as_ref(), Some(&team));
        assert_eq!(snapshot.logo_path.as_deref(), Some(logo_path));
        assert!(!snapshot.offline);
        {
            let state = service.state();
            assert!(
                state.accepted.is_none(),
                "old Binding frame must never become ready"
            );
            assert_eq!(
                state.confirms, 1,
                "no synthetic Waiting frame may be confirmed"
            );
        }

        // A separate presentation event, not the successful snapshot read,
        // supplies frame evidence for the authoritative Waiting revision.
        assert_eq!(
            capture.events().len(),
            usize::from(screen == SessionScreenKind::BindingPending)
        );
        timing.queued(snapshot.ui_revision, Instant::now());
        let waiting_frame = SessionPresentation {
            agent_lease_id: snapshot.agent_lease_id.clone(),
            session: snapshot.session.clone(),
            ui_revision: snapshot.ui_revision,
            ..frame
        };
        assert_confirmed(&proxy, &lease, deadline, &waiting_frame, &mut timing).await;
        let state = service.state();
        let events = capture.events();
        assert_frame(
            events
                .last()
                .unwrap_or_else(|| panic!("current frame must be logged")),
            waiting_frame.ui_revision,
        );
        assert_eq!(state.accepted.as_ref(), Some(&waiting_frame));
        assert_eq!(state.lease, lease);
        assert_eq!(state.confirms, 2);
        assert_eq!(
            state.gets, 2,
            "acceptance must not fetch or replace the snapshot"
        );
    }
}

const CONFIRMATION_REJECTIONS: [&str; 17] = [
    "expired",
    "replaced",
    "authentication",
    "access_denied",
    "auth_failed",
    "current_revision",
    "regressed_revision",
    "snapshot_session",
    "snapshot_lease",
    "get_authentication",
    "get_expired",
    "get_replaced",
    "get_invalid",
    "invalid_frame",
    "frame_lease",
    "frame_session",
    "other_error",
];

#[tokio::test]
async fn rejected_confirmation_negative_controls_fail_closed() {
    let (_guard, capture, dispatch) = capture();
    let _subscriber = tracing::dispatcher::set_default(&dispatch);
    // Even if a newer snapshot could be fetched, only the known stale-frame
    // rejection may enter recovery. Authentication and lease errors are terminal.
    for case in CONFIRMATION_REJECTIONS {
        let (service, lease, mut frame) = fixture();
        let mut timing = PresentationTiming::new(frame.ui_revision, Instant::now());
        let expected_gets = match case {
            "expired" | "replaced" | "authentication" | "access_denied" | "auth_failed"
            | "invalid_frame" | "frame_lease" | "frame_session" | "other_error" => 0,
            _ => 1,
        };
        {
            let mut state = service.state();
            match case {
                "expired" => state.live = false,
                "replaced" => state.lease.lease_id = "replacement".into(),
                "authentication" => {
                    state.confirm_error = Some(zbus::fdo::Error::Failed(
                        "Session Agent caller was rejected".into(),
                    ));
                }
                "access_denied" => {
                    state.confirm_error = Some(zbus::fdo::Error::AccessDenied(
                        "Session presentation is incomplete or stale".into(),
                    ));
                }
                "auth_failed" => {
                    state.confirm_error = Some(zbus::fdo::Error::AuthFailed(
                        "Session presentation is incomplete or stale".into(),
                    ));
                }
                "current_revision" => {
                    state.confirm_error = Some(zbus::fdo::Error::Failed(
                        "Session presentation is incomplete or stale".into(),
                    ));
                    state.snapshot.ui_revision = frame.ui_revision;
                }
                "regressed_revision" => state.snapshot.ui_revision = frame.ui_revision - 1,
                "snapshot_session" => state.snapshot.session.logind_session_id = "different".into(),
                "snapshot_lease" => state.snapshot.agent_lease_id = "different".into(),
                "get_authentication" => state.get_error = Some("Session Agent caller was rejected"),
                "get_expired" | "get_replaced" => {
                    state.get_error = Some("Session Agent lease is stale");
                }
                "get_invalid" => state.get_error = Some("Session UI state is unavailable"),
                "invalid_frame" => frame.fullscreen_width = 0,
                "frame_lease" => frame.agent_lease_id = "different".into(),
                "frame_session" => frame.session.logind_session_id = "different".into(),
                "other_error" => {
                    state.confirm_error = Some(zbus::fdo::Error::Failed(
                        "Session UI state is unavailable".into(),
                    ));
                }
                _ => unreachable!(),
            }
        }
        let (_server, client) = peer(service.clone()).await;
        let proxy = Device1Proxy::new(&client)
            .await
            .unwrap_or_else(|error| panic!("proxy: {error}"));
        assert!(
            matches!(
                confirm_presentation(
                    &proxy,
                    &lease,
                    Instant::now() + Duration::from_secs(2),
                    &frame,
                    &mut timing
                )
                .await,
                Err(ConnectionEnd::Disconnected(_))
            ),
            "case {case} must remain terminal"
        );
        let state = service.state();
        assert!(
            state.accepted.is_none(),
            "case {case} must not become ready"
        );
        assert_eq!(
            state.confirms, 1,
            "case {case} must keep exactly one confirmation"
        );
        assert_eq!(state.gets, expected_gets, "case {case}");
        assert!(
            capture.events().is_empty(),
            "case {case} must not log a frame"
        );
    }
}

#[tokio::test]
async fn recovery_snapshot_cannot_outlive_existing_deadline() {
    let (_guard, capture, dispatch) = capture();
    let _subscriber = tracing::dispatcher::set_default(&dispatch);
    let (service, lease, frame) = fixture();
    let mut timing = PresentationTiming::new(frame.ui_revision, Instant::now());
    service.state().get_delay = Duration::from_secs(5);
    let (_server, client) = peer(service.clone()).await;
    let proxy = Device1Proxy::new(&client)
        .await
        .unwrap_or_else(|error| panic!("proxy: {error}"));
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        confirm_presentation(
            &proxy,
            &lease,
            Instant::now() + Duration::from_millis(100),
            &frame,
            &mut timing,
        ),
    )
    .await;
    assert!(matches!(result, Ok(Err(ConnectionEnd::Disconnected(_)))));
    let state = service.state();
    assert_eq!(state.confirms, 1);
    assert_eq!(state.gets, 1);
    assert!(state.accepted.is_none());
    assert!(
        capture.events().is_empty(),
        "timed out recovery must not log a frame"
    );
}

#[tokio::test]
async fn expired_deadline_prevents_confirmation_or_recovery() {
    let (_guard, capture, dispatch) = capture();
    let _subscriber = tracing::dispatcher::set_default(&dispatch);
    let (service, lease, frame) = fixture();
    let mut timing = PresentationTiming::new(frame.ui_revision, Instant::now());
    let (_server, client) = peer(service.clone()).await;
    let proxy = Device1Proxy::new(&client)
        .await
        .unwrap_or_else(|error| panic!("proxy: {error}"));
    assert!(matches!(
        confirm_presentation(&proxy, &lease, Instant::now(), &frame, &mut timing).await,
        Err(ConnectionEnd::Disconnected(_))
    ));
    let state = service.state();
    assert_eq!(state.confirms, 0);
    assert_eq!(state.gets, 0);
    assert!(state.accepted.is_none());
    assert!(
        capture.events().is_empty(),
        "expired confirmation must not log a frame"
    );
}

#[tokio::test]
async fn first_frame_is_logged_once_across_duplicates_withdrawal_and_renewal() {
    let (_guard, capture, dispatch) = capture();
    let _subscriber = tracing::dispatcher::set_default(&dispatch);
    let (service, mut lease, mut frame) = fixture();
    lease.expires_at_unix_ms -= 1;
    {
        let mut state = service.state();
        state.lease = lease.clone();
        state.snapshot.ui_revision = frame.ui_revision;
    }
    let (_server, client) = peer(service.clone()).await;
    let proxy = Device1Proxy::new(&client)
        .await
        .unwrap_or_else(|error| panic!("proxy: {error}"));
    let registered_at = Instant::now() - Duration::from_millis(100);
    let mut timing = PresentationTiming::new(frame.ui_revision, registered_at);
    timing.queued(frame.ui_revision, registered_at + Duration::from_millis(50));
    let deadline = Instant::now() + Duration::from_secs(2);
    let withdrawal = SessionPresentation {
        first_frame_presented: false,
        fullscreen_width: 0,
        fullscreen_height: 0,
        ..frame.clone()
    };
    assert_confirmed(&proxy, &lease, deadline, &withdrawal, &mut timing).await;
    assert!(
        capture.events().is_empty(),
        "withdrawal cannot consume the first frame event"
    );
    for _ in 0..2 {
        assert_confirmed(&proxy, &lease, deadline, &frame, &mut timing).await;
    }
    assert_eq!(
        capture.events().len(),
        1,
        "duplicate success must stay silent"
    );
    assert_frame(&capture.events()[0], frame.ui_revision);
    assert_eq!(service.state().accepted.as_ref(), Some(&frame));

    assert_confirmed(&proxy, &lease, deadline, &withdrawal, &mut timing).await;
    assert!(service.state().accepted.is_none());
    assert_eq!(capture.events().len(), 1, "withdrawal must stay silent");
    assert_confirmed(&proxy, &lease, deadline, &frame, &mut timing).await;
    assert_eq!(
        capture.events().len(),
        1,
        "redraw after withdrawal must stay silent"
    );

    lease.expires_at_unix_ms += 1;
    service.state().lease = lease.clone();
    assert_confirmed(&proxy, &lease, deadline, &frame, &mut timing).await;
    assert_eq!(timing.registered_at, registered_at);
    assert_eq!(
        capture.events().len(),
        1,
        "renewing the same lease must stay silent"
    );

    frame.ui_revision += 1;
    service.state().snapshot.ui_revision = frame.ui_revision;
    timing.queued(frame.ui_revision, Instant::now());
    assert_eq!(
        timing.registered_at, registered_at,
        "a revision must retain the lease start"
    );
    assert_confirmed(&proxy, &lease, deadline, &frame, &mut timing).await;
    assert_eq!(capture.events().len(), 2);
    assert_frame(&capture.events()[1], frame.ui_revision);

    lease.lease_id = "new-registration-canary".into();
    frame.agent_lease_id = lease.lease_id.clone();
    {
        let mut state = service.state();
        state.lease = lease.clone();
        state.snapshot.agent_lease_id = lease.lease_id.clone();
    }
    let new_registered_at = Instant::now();
    let mut timing = PresentationTiming::new(frame.ui_revision, new_registered_at);
    assert_confirmed(&proxy, &lease, deadline, &frame, &mut timing).await;
    assert_eq!(timing.registered_at, new_registered_at);
    let events = capture.events();
    assert_eq!(
        events.len(),
        3,
        "a new registration may report the same revision"
    );
    assert_frame(&events[2], frame.ui_revision);
    let encoded = format!("{events:?}");
    for canary in [
        "existing-lease",
        "new-registration-canary",
        &lease.session.boot_id,
    ] {
        assert!(!encoded.contains(canary), "identity must not be logged");
    }
    let state = service.state();
    assert_eq!(
        state.confirms, 8,
        "logging must not skip or add confirmations"
    );
    assert_eq!(state.gets, 0);
    assert_eq!(state.accepted.as_ref(), Some(&frame));
}

#[test]
fn timing_uses_local_lease_and_revision_origins_and_ignores_stale_or_withdrawn_frames() {
    let (_guard, capture, dispatch) = capture();
    let _subscriber = tracing::dispatcher::set_default(&dispatch);
    let (_, _, mut frame) = fixture();
    let start = Instant::now();
    let mut timing = PresentationTiming::new(frame.ui_revision, start);
    timing.queued(frame.ui_revision, start + Duration::from_micros(10));
    let withdrawal = SessionPresentation {
        first_frame_presented: false,
        ..frame.clone()
    };
    timing.confirmed(&withdrawal, start + Duration::from_micros(20));
    let stale = SessionPresentation {
        ui_revision: frame.ui_revision - 1,
        ..frame.clone()
    };
    timing.confirmed(&stale, start + Duration::from_micros(30));
    assert!(capture.events().is_empty());
    timing.confirmed(&frame, start + Duration::from_micros(40));
    timing.confirmed(&frame, start + Duration::from_micros(50));
    let events = capture.events();
    assert_eq!(events.len(), 1);
    assert_frame(&events[0], frame.ui_revision);
    assert_eq!(
        events[0].fields.get("since_lease_us"),
        Some(&Value::Unsigned(40))
    );
    assert_eq!(
        events[0].fields.get("since_revision_us"),
        Some(&Value::Unsigned(30))
    );

    frame.ui_revision += 1;
    timing.queued(frame.ui_revision, start + Duration::from_micros(60));
    timing.confirmed(&stale, start + Duration::from_micros(70));
    assert_eq!(capture.events().len(), 1);
    timing.confirmed(&frame, start + Duration::from_micros(80));
    let events = capture.events();
    assert_eq!(events.len(), 2);
    assert_frame(&events[1], frame.ui_revision);
    assert_eq!(
        events[1].fields.get("since_lease_us"),
        Some(&Value::Unsigned(80))
    );
    assert_eq!(
        events[1].fields.get("since_revision_us"),
        Some(&Value::Unsigned(20))
    );
}
