use std::{collections::BTreeMap, fmt, sync::Mutex};

use tracing::{Event, Level, Subscriber, field::Visit, instrument::WithSubscriber as _};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

use super::*;
use crate::device_control::{ConnectionEnd, DispatchCause, DispatchMetadata, OutboundSnapshot};
use crate::logging::tests::SubscriberTestGuard;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    Text(String),
    Unsigned(u64),
    Boolean(bool),
}

#[derive(Clone, Debug)]
struct Trace {
    message: String,
    level: Level,
    fields: BTreeMap<String, Value>,
}

#[derive(Clone, Default)]
struct Traces(Arc<Mutex<Vec<Trace>>>);

#[derive(Default)]
struct Fields(BTreeMap<String, Value>);

impl Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        self.0
            .insert(field.name().to_owned(), Value::Text(format!("{value:?}")));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0
            .insert(field.name().to_owned(), Value::Text(value.to_owned()));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0
            .insert(field.name().to_owned(), Value::Unsigned(value));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0
            .insert(field.name().to_owned(), Value::Boolean(value));
    }
}

impl<S: Subscriber> Layer<S> for Traces {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let Some(Value::Text(message)) = fields.0.remove("message") else {
            return;
        };
        if !matches!(message.as_str(), "target dispatched" | "actual received") {
            return;
        }
        self.0
            .lock()
            .unwrap_or_else(|_| panic!("trace capture poisoned"))
            .push(Trace {
                message,
                level: *event.metadata().level(),
                fields: fields.0,
            });
    }
}

impl Traces {
    fn events(&self, message: &str) -> Vec<Trace> {
        self.0
            .lock()
            .unwrap_or_else(|_| panic!("trace capture poisoned"))
            .iter()
            .filter(|event| event.message == message)
            .cloned()
            .collect()
    }
}

fn canonical_outbound(device_id: DeviceId, session: Uuid) -> OutboundSnapshot {
    let target = natsume_workstation_protocol::generated::ConcreteTargetState::decode(
        include_bytes!("../../../../natsume-workstation-protocol/testdata/target-digest.pb")
            .as_slice(),
    )
    .unwrap_or_else(|error| panic!("canonical target: {error}"));
    let credential_id = target
        .gateway
        .as_ref()
        .unwrap_or_else(|| panic!("canonical Gateway missing"))
        .credential_id
        .clone();
    OutboundSnapshot {
        envelope: ServerActiveEnvelope {
            session_id: session.as_bytes().to_vec(),
            body: Some(server_active_envelope::Body::ServerState(
                natsume_workstation_protocol::generated::ServerStateSnapshot {
                    intent: Some(natsume_workstation_protocol::generated::ServerIntentState {
                        gateway_credential: Some(
                            natsume_workstation_protocol::generated::GatewayCredentialIntent {
                                credential_id,
                            },
                        ),
                        binding: None,
                    }),
                    target: Some(target),
                },
            )),
        },
        metadata: DispatchMetadata {
            device_id,
            cause: DispatchCause::Dirty,
            queued: Some(Duration::from_micros(5)),
            materialize: Duration::from_micros(17),
        },
    }
}

async fn active_socket(
    handle: DeviceHandle,
    session: Uuid,
) -> (
    TestSocket,
    mpsc::Sender<OutboundSnapshot>,
    tokio::sync::oneshot::Receiver<ConnectionEnd>,
    tokio::task::JoinHandle<()>,
) {
    let (outbound, outgoing) = mpsc::channel(super::super::OUTBOUND_CAPACITY);
    let (ended, ending) = tokio::sync::oneshot::channel();
    let resources = Arc::new(tokio::sync::Mutex::new(Some((outgoing, ended))));
    let application = axum::Router::new().route(
        "/",
        axum::routing::get(move |upgrade: axum::extract::WebSocketUpgrade| {
            let handle = handle.clone();
            let resources = Arc::clone(&resources);
            async move {
                let (mut outgoing, ended) = resources
                    .lock()
                    .await
                    .take()
                    .unwrap_or_else(|| panic!("socket fixture already used"));
                upgrade.on_upgrade(move |socket| async move {
                    let cause =
                        super::super::run_active(socket, session, &handle, &mut outgoing).await;
                    let _ = ended.send(cause);
                })
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("listener: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("address: {error}"));
    let server = tokio::spawn(
        async move {
            let _ = axum::serve(listener, application).await;
        }
        .with_current_subscriber(),
    );
    let (socket, _) = connect_async(format!("ws://{address}/"))
        .await
        .unwrap_or_else(|error| panic!("socket: {error}"));
    (socket, outbound, ending, server)
}

#[tokio::test]
async fn dispatched_event_matches_the_canonical_target_written_to_the_socket() {
    let _guard = SubscriberTestGuard::acquire();
    let traces = Traces::default();
    let _subscriber =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(traces.clone()));
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x71; 32])).await;
    let handle = fixture
        .runtime
        .device_control()
        .registry
        .get_or_spawn(device_id)
        .await
        .unwrap_or_else(|| panic!("actor missing"));
    let session = Uuid::now_v7();
    let (mut socket, outbound, ending, server) = active_socket(handle, session).await;
    assert!(
        outbound
            .try_send(canonical_outbound(device_id, session))
            .is_ok()
    );
    drop(outbound);
    let envelope = ServerActiveEnvelope::decode(receive_binary(&mut socket).await.as_slice())
        .unwrap_or_else(|error| panic!("envelope: {error}"));
    assert!(matches!(
        ending.await.unwrap_or_else(|error| panic!("pump: {error}")),
        ConnectionEnd::OutgoingClosed
    ));
    let Some(server_active_envelope::Body::ServerState(snapshot)) = envelope.body else {
        panic!("missing ServerState");
    };
    let target = snapshot.target.unwrap_or_else(|| panic!("missing target"));
    let digest = natsume_workstation_protocol::target_digest(&target);
    assert_eq!(
        digest,
        include_str!("../../../../natsume-workstation-protocol/testdata/target-digest.txt")
            .trim_end()
    );
    let events = traces.events("target dispatched");
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.level, Level::INFO);
    for (field, value) in [
        ("device_id", Value::Text(device_id.as_text())),
        ("target_digest", Value::Text(digest)),
        ("foreground_target", Value::Text("contest".to_owned())),
        ("terminate_epoch", Value::Unsigned(7)),
        ("reset_epoch", Value::Unsigned(9)),
        ("cause", Value::Text("dirty".to_owned())),
        ("queued_us", Value::Unsigned(5)),
        ("materialize_us", Value::Unsigned(17)),
    ] {
        assert_eq!(event.fields.get(field), Some(&value), "field {field}");
    }
    assert!(matches!(
        event.fields.get("send_us"),
        Some(Value::Unsigned(_))
    ));
    let fields = format!("{:?}", event.fields);
    for forbidden in [
        "fictitious-observability-password-canary",
        "fixture-team",
        "A-01",
        "Fixture Team",
        "https://judge.example.test",
    ] {
        assert!(!fields.contains(forbidden), "leaked {forbidden}");
    }
    server.abort();
}

#[tokio::test]
async fn a_closed_transport_does_not_emit_a_dispatch_event() {
    let _guard = SubscriberTestGuard::acquire();
    let traces = Traces::default();
    let _subscriber =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(traces.clone()));
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x72; 32])).await;
    let handle = fixture
        .runtime
        .device_control()
        .registry
        .get_or_spawn(device_id)
        .await
        .unwrap_or_else(|| panic!("actor missing"));
    let session = Uuid::now_v7();
    let (mut socket, outbound, ending, server) = active_socket(handle, session).await;
    socket
        .close(None)
        .await
        .unwrap_or_else(|error| panic!("close: {error}"));
    assert!(matches!(
        ending.await.unwrap_or_else(|error| panic!("pump: {error}")),
        ConnectionEnd::ClientClose
    ));
    assert!(
        outbound
            .try_send(canonical_outbound(device_id, session))
            .is_err()
    );
    assert!(traces.events("target dispatched").is_empty());
    server.abort();
}

async fn accept_snapshot(
    fixture: &Fixture,
    device_id: DeviceId,
    handle: &DeviceHandle,
    session: Uuid,
    snapshot: ClientStateSnapshot,
    outgoing: &mut mpsc::Receiver<OutboundSnapshot>,
) {
    assert!(handle.enqueue_client_state(session, snapshot).await.is_ok());
    let outbound = timeout(Duration::from_secs(5), outgoing.recv())
        .await
        .unwrap_or_else(|_| panic!("snapshot timeout"))
        .unwrap_or_else(|| panic!("lease closed"));
    assert!(matches!(
        outbound.metadata.cause,
        DispatchCause::ClientState
    ));
    assert!(outbound.metadata.queued.is_none());
    let _ = fixture
        .runtime
        .device_control()
        .registry
        .read_connection_state(device_id)
        .await;
}

#[tokio::test]
async fn actual_event_changed_matches_journal_summaries_and_excludes_rejected_frames() {
    let _guard = SubscriberTestGuard::acquire();
    let traces = Traces::default();
    let _subscriber =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(traces.clone()));
    let mut recorded = journal::RecordedFixture::new().await;
    let fixture = &recorded.fixture;
    let control = fixture.runtime.device_control();
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x73; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(control, device_id, outbound).await;
    let mut snapshot = valid_snapshot();
    for (index, expected_changes) in [7, 0, 0, 1].into_iter().enumerate() {
        if index == 2 {
            let actual = snapshot.actual.as_mut().unwrap_or_else(|| panic!("actual"));
            let session = actual
                .session_control
                .as_mut()
                .unwrap_or_else(|| panic!("session"));
            session.waiting_ready = true;
            session.completed_terminate_epoch = Some(7);
            actual
                .home
                .as_mut()
                .unwrap_or_else(|| panic!("home"))
                .completed_reset_epoch = Some(9);
        } else if index == 3 {
            let session = snapshot
                .actual
                .as_mut()
                .and_then(|actual| actual.session_control.as_mut())
                .unwrap_or_else(|| panic!("session"));
            session.session_state = SessionState::Running.into();
            session.contest_ready = true;
        }
        accept_snapshot(
            fixture,
            device_id,
            &handle,
            session,
            snapshot.clone(),
            &mut outgoing,
        )
        .await;
        let changes = crate::journal::tests::drain_events(&mut recorded.receiver)
            .into_iter()
            .filter(|event| matches!(event.event, crate::journal::event::Event::ActualChanged(_)))
            .count();
        assert_eq!(changes, expected_changes);
        let events = traces.events("actual received");
        assert_eq!(events.len(), index + 1);
        let event = &events[index];
        assert_eq!(event.level, Level::INFO);
        assert_eq!(
            event.fields.get("changed"),
            Some(&Value::Boolean(changes != 0))
        );
        assert!(matches!(
            event.fields.get("reconcile_us"),
            Some(Value::Unsigned(_))
        ));
    }
    let events = traces.events("actual received");
    let event = &events[3];
    for (field, value) in [
        ("device_id", Value::Text(device_id.as_text())),
        ("session_foreground", Value::Text("unknown".to_owned())),
        ("session_state", Value::Text("running".to_owned())),
        ("home_state", Value::Text("steady".to_owned())),
        ("waiting_ready", Value::Boolean(true)),
        ("contest_ready", Value::Boolean(true)),
        ("completed_terminate_epoch", Value::Unsigned(7)),
        ("completed_reset_epoch", Value::Unsigned(9)),
    ] {
        assert_eq!(event.fields.get(field), Some(&value), "field {field}");
    }
    assert!(traces.events("target dispatched").is_empty());
    assert!(
        handle
            .enqueue_client_state(Uuid::nil(), snapshot)
            .await
            .is_ok()
    );
    let _ = control.registry.read_connection_state(device_id).await;
    assert_eq!(traces.events("actual received").len(), 4);
    assert!(
        handle
            .enqueue_client_state(session, ClientStateSnapshot::default())
            .await
            .is_ok()
    );
    assert!(
        timeout(Duration::from_secs(5), outgoing.recv())
            .await
            .unwrap_or_else(|_| panic!("rejection timeout"))
            .is_none()
    );
    assert_eq!(traces.events("actual received").len(), 4);
}

#[tokio::test]
async fn backpressure_does_not_publish_a_second_actual_observation() {
    let _guard = SubscriberTestGuard::acquire();
    let traces = Traces::default();
    let _subscriber =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(traces.clone()));
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x74; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) =
        replace_current_lease(fixture.runtime.device_control(), device_id, outbound).await;
    for _ in 0..2 {
        assert!(
            handle
                .enqueue_client_state(session, valid_snapshot())
                .await
                .is_ok()
        );
        let _ = fixture
            .runtime
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await;
    }
    assert_eq!(traces.events("actual received").len(), 1);
    assert!(traces.events("target dispatched").is_empty());
    assert!(outgoing.recv().await.is_some());
    assert!(outgoing.recv().await.is_none());
}

#[tokio::test]
async fn transient_ingest_failure_emits_actual_only_after_successful_retry() {
    let _guard = SubscriberTestGuard::acquire();
    let traces = Traces::default();
    let _subscriber =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(traces.clone()));
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x75; 32])).await;
    let control = fixture.runtime.device_control();
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(control, device_id, outbound).await;
    let failures = fixture.query_failures();
    let lock = crate::db::tests::test_lock_database(&fixture.path)
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    assert!(
        handle
            .enqueue_client_state(session, valid_snapshot())
            .await
            .is_ok()
    );
    await_query_failure(&failures).await;
    assert!(matches!(
        control.registry.read_connection_state(device_id).await,
        DeviceConnectionState::AwaitingFreshState
    ));
    assert!(traces.events("actual received").is_empty());
    assert!(outgoing.is_empty());
    drop(lock);
    let outbound = timeout(
        TRANSIENT_RETRY_DELAY + Duration::from_secs(2),
        outgoing.recv(),
    )
    .await
    .unwrap_or_else(|_| panic!("retry timeout"))
    .unwrap_or_else(|| panic!("retry closed lease"));
    assert!(matches!(outbound.metadata.cause, DispatchCause::Retry));
    assert!(outbound.metadata.queued.is_none());
    let _ = control.registry.read_connection_state(device_id).await;
    let events = traces.events("actual received");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].fields.get("changed"), Some(&Value::Boolean(true)));
}
