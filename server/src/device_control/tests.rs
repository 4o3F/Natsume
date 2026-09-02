use std::{collections::HashSet, fs, path::PathBuf, sync::Arc, time::Duration};

use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt as _, StreamExt as _};
use natsume_device_protocol::{
    CONTROL_ROUTE, CONTROL_SUBPROTOCOL,
    generated::{
        ActualState, BindingAccessActualState, BindingArtifactState, ClientActiveEnvelope,
        ClientHandshakeEnvelope, ClientInputState, ClientProof, ClientStateSnapshot,
        EnrollmentAttempt, EnrollmentEvidenceQuality, GatewayActualState, GatewayState,
        HomeActualState, HomeState, RuntimeConfigActualState, RuntimeConfigState,
        ServerActiveEnvelope, ServerHandshakeEnvelope, SessionControlActualState, SessionState,
        client_active_envelope, client_handshake_envelope, client_proof, server_active_envelope,
        server_handshake_envelope,
    },
    sign_client_proof,
};
use prost::Message as _;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message as ClientMessage, client::IntoClientRequest as _},
};
use uuid::Uuid;

use crate::{
    component::device::{
        ControlPublicKey, DeviceId, EnrollmentStartOutcome, EvidenceQuality, MachineHardwareId,
        ValidatedEnrollmentEvidence,
    },
    db::{Database, DatabaseConfig, PersistenceError},
    diesel_schema::{
        binding_negotiations, device_home_targets, device_session_targets, gateway_credentials,
        runtime_config,
    },
    server_state::{self, ServerState},
};

use super::{DeviceRegistry, actor::DeviceHandle};

const MACHINE_HARDWARE_ID: &str = "a9aa9d04-3ece-5567-8260-910930ff5e03";
const DOMJUDGE_ORIGIN: &str = "https://domjudge.example.test";

#[tokio::test]
async fn actor_rejects_stale_and_invalid_frames_before_component_writes() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x61; 32])).await;

    let (outbound, mut first_outgoing) = mpsc::channel(1);
    let (first_session, handle) = attach(&fixture.state, device_id, outbound).await;
    let (outbound, _second_outgoing) = mpsc::channel(1);
    let (_second_session, _) = attach(&fixture.state, device_id, outbound).await;
    assert!(first_outgoing.recv().await.is_none());

    assert!(
        handle
            .client_state(Arc::clone(&fixture.state), first_session, valid_snapshot(),)
            .await
    );
    let (outbound, _third_outgoing) = mpsc::channel(1);
    let (third_session, _) = attach(&fixture.state, device_id, outbound).await;
    assert_eq!(fixture.component_row_counts().await, [0, 0, 0, 0]);

    assert!(
        handle
            .client_state(
                Arc::clone(&fixture.state),
                third_session,
                ClientStateSnapshot {
                    input: None,
                    actual: None,
                },
            )
            .await
    );
    let (outbound, mut current_outgoing) = mpsc::channel(1);
    let (current_session, _) = attach(&fixture.state, device_id, outbound).await;
    assert_eq!(fixture.component_row_counts().await, [0, 0, 0, 0]);

    assert!(
        handle
            .client_state(
                Arc::clone(&fixture.state),
                current_session,
                valid_snapshot(),
            )
            .await
    );
    let envelope = timeout(Duration::from_secs(5), current_outgoing.recv())
        .await
        .unwrap_or_else(|_| panic!("the actor did not materialize the valid first snapshot"))
        .unwrap_or_else(|| panic!("the actor closed the valid lease"));
    assert_complete_target(envelope);
    assert_eq!(fixture.component_row_counts().await, [1, 1, 1, 1]);
}

#[tokio::test]
async fn full_outbound_queue_terminates_the_current_lease() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x62; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = attach(&fixture.state, device_id, outbound).await;

    assert!(
        handle
            .client_state(Arc::clone(&fixture.state), session_id, valid_snapshot())
            .await
    );
    timeout(Duration::from_secs(5), async {
        while outgoing.len() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the first target did not fill the outbound queue"));
    assert!(
        handle
            .client_state(Arc::clone(&fixture.state), session_id, valid_snapshot())
            .await
    );

    let (replacement_outbound, _replacement_outgoing) = mpsc::channel(1);
    let _ = attach(&fixture.state, device_id, replacement_outbound).await;
    assert!(outgoing.recv().await.is_some());
    assert!(outgoing.recv().await.is_none());
}

#[tokio::test]
async fn registry_accepts_six_hundred_devices() {
    let registry = DeviceRegistry::new();
    let mut sessions = HashSet::new();
    let mut outgoing = Vec::with_capacity(600);

    timeout(Duration::from_secs(10), async {
        for _ in 0..600 {
            let device_id = DeviceId::parse(&Uuid::now_v7().hyphenated().to_string())
                .unwrap_or_else(|| panic!("a generated UUIDv7 was not a Device ID"));
            let (outbound, receiver) = mpsc::channel(1);
            let (session_id, _) = registry
                .attach(device_id, outbound)
                .await
                .unwrap_or_else(|| panic!("the registry rejected a Device"));
            assert!(sessions.insert(session_id));
            outgoing.push(receiver);
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the registry did not accept 600 Devices in time"));
    assert_eq!(sessions.len(), 600);
}

#[tokio::test]
async fn production_websocket_delivers_enrollment_and_a_complete_target() {
    let fixture = Fixture::new().await;
    fixture.state.provisioning().open_window().await;
    let (mut socket, server) = connect(&fixture).await;
    let session_id = enroll(&fixture, &mut socket).await;

    socket
        .send(ClientMessage::Binary(
            ClientActiveEnvelope {
                session_id,
                body: Some(client_active_envelope::Body::ClientState(valid_snapshot())),
            }
            .encode_to_vec()
            .into(),
        ))
        .await
        .unwrap_or_else(|error| panic!("ClientState send failed: {error}"));
    let bytes = receive_binary(&mut socket).await;
    let active = ServerActiveEnvelope::decode(bytes.as_slice())
        .unwrap_or_else(|error| panic!("ServerActive decode failed: {error}"));
    assert_complete_target(active);

    drop(socket);
    server.abort();
}

async fn connect(fixture: &Fixture) -> (TestSocket, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("test listener failed: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("test listener address failed: {error}"));
    let application = crate::http::router(
        Arc::clone(&fixture.state),
        std::path::Path::new("/natsume-wss-test-unused-web-root"),
    );
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, application).await;
    });

    let mut request = format!("ws://{address}{CONTROL_ROUTE}")
        .into_client_request()
        .unwrap_or_else(|error| panic!("WebSocket request build failed: {error}"));
    request.headers_mut().insert(
        "sec-websocket-protocol",
        CONTROL_SUBPROTOCOL
            .parse()
            .unwrap_or_else(|error| panic!("subprotocol header failed: {error}")),
    );
    let (socket, response) = connect_async(request)
        .await
        .unwrap_or_else(|error| panic!("WebSocket connect failed: {error}"));
    assert_eq!(response.status(), 101);
    assert_eq!(
        response
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok()),
        Some(CONTROL_SUBPROTOCOL)
    );
    (socket, server)
}

async fn enroll(fixture: &Fixture, socket: &mut TestSocket) -> Vec<u8> {
    let Some(server_handshake_envelope::Body::ServerChallenge(challenge)) =
        receive_handshake(socket).await.body
    else {
        panic!("Server did not begin with a challenge");
    };
    let signing_key = SigningKey::from_bytes(&[0x63; 32]);
    let proof = sign_client_proof(
        &signing_key,
        &challenge,
        ClientProof {
            daemon_version: "2.0.0".to_owned(),
            agent_version: "2.0.0".to_owned(),
            machine_hardware_id: MACHINE_HARDWARE_ID.to_owned(),
            signature: Vec::new(),
            purpose: Some(client_proof::Purpose::Enrollment(EnrollmentAttempt {
                candidate_public_key: signing_key.verifying_key().to_bytes().to_vec(),
                evidence_quality: EnrollmentEvidenceQuality::Strong.into(),
            })),
        },
    );
    send_handshake(
        socket,
        ClientHandshakeEnvelope {
            body: Some(client_handshake_envelope::Body::ClientProof(proof)),
        },
    )
    .await;
    assert!(matches!(
        receive_handshake(socket).await.body,
        Some(server_handshake_envelope::Body::EnrollmentReviewStatus(_))
    ));

    let reviews = fixture.state.device().pending_enrollment_reviews().await;
    assert_eq!(reviews.len(), 1);
    let authority = fixture
        .state
        .device()
        .approve_enrollment(fixture.state.provisioning(), reviews[0].review_id())
        .await
        .unwrap_or_else(|error| panic!("Enrollment approval failed: {error:?}"));
    let Some(server_handshake_envelope::Body::EnrollmentActivated(enrollment_authority)) =
        receive_handshake(socket).await.body
    else {
        panic!("Server did not deliver the committed Enrollment authority");
    };
    assert_eq!(
        enrollment_authority.device_id,
        authority.device_id().as_text()
    );
    send_handshake(
        socket,
        ClientHandshakeEnvelope {
            body: Some(client_handshake_envelope::Body::EnrollmentReady(
                enrollment_authority,
            )),
        },
    )
    .await;
    let Some(server_handshake_envelope::Body::SessionReady(ready)) =
        receive_handshake(socket).await.body
    else {
        panic!("Server did not establish the active session");
    };
    assert_eq!(ready.session_id.len(), 16);
    ready.session_id
}

async fn attach(
    state: &Arc<ServerState>,
    device_id: DeviceId,
    outbound: mpsc::Sender<ServerActiveEnvelope>,
) -> ([u8; 16], DeviceHandle) {
    state
        .device_registry()
        .attach(device_id, outbound)
        .await
        .unwrap_or_else(|| panic!("the Device registry rejected an attach"))
}

fn valid_snapshot() -> ClientStateSnapshot {
    ClientStateSnapshot {
        input: Some(ClientInputState {
            gateway_credential: None,
            binding: None,
        }),
        actual: Some(ActualState {
            gateway: Some(GatewayActualState {
                credential_id: None,
                state: GatewayState::Absent.into(),
                gateway_leaf_sha256: None,
            }),
            binding_access: Some(BindingAccessActualState {
                assignment_state: BindingArtifactState::Absent.into(),
                credential_state: BindingArtifactState::Absent.into(),
                context: None,
            }),
            runtime_config: Some(RuntimeConfigActualState {
                state: RuntimeConfigState::Absent.into(),
                applied_domjudge_origin: None,
            }),
            session_control: Some(SessionControlActualState {
                session_state: SessionState::None.into(),
                completed_terminate_epoch: None,
            }),
            home: Some(HomeActualState {
                state: HomeState::Steady.into(),
                completed_reset_epoch: None,
            }),
        }),
    }
}

fn assert_complete_target(envelope: ServerActiveEnvelope) {
    let Some(server_active_envelope::Body::ServerState(snapshot)) = envelope.body else {
        panic!("the Actor did not emit ServerState");
    };
    let intent = snapshot
        .intent
        .unwrap_or_else(|| panic!("Server intent was absent"));
    assert!(intent.gateway_credential.is_some());
    assert!(intent.binding.is_some());
    let target = snapshot
        .target
        .unwrap_or_else(|| panic!("concrete target was absent"));
    assert!(target.gateway.is_some());
    assert!(target.binding_access.is_some());
    assert!(target.runtime_config.is_some());
    assert!(target.session_control.is_some());
    assert!(target.home.is_some());
}

type TestSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn send_handshake(socket: &mut TestSocket, envelope: ClientHandshakeEnvelope) {
    socket
        .send(ClientMessage::Binary(envelope.encode_to_vec().into()))
        .await
        .unwrap_or_else(|error| panic!("Client handshake send failed: {error}"));
}

async fn receive_handshake(socket: &mut TestSocket) -> ServerHandshakeEnvelope {
    let bytes = receive_binary(socket).await;
    ServerHandshakeEnvelope::decode(bytes.as_slice())
        .unwrap_or_else(|error| panic!("Server handshake decode failed: {error}"))
}

async fn receive_binary(socket: &mut TestSocket) -> Vec<u8> {
    match timeout(Duration::from_secs(5), socket.next()).await {
        Ok(Some(Ok(ClientMessage::Binary(bytes)))) => bytes.to_vec(),
        result => panic!("Server did not send a binary WebSocket message: {result:?}"),
    }
}

struct Fixture {
    path: PathBuf,
    database: Database,
    state: Arc<ServerState>,
}

impl Fixture {
    async fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "natsume-device-control-test-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .unwrap_or_else(|error| panic!("test database creation failed: {error:?}"));
        database
            .write(|transaction| {
                diesel::insert_into(runtime_config::table)
                    .values((
                        runtime_config::singleton.eq(1),
                        runtime_config::domjudge_origin.eq(DOMJUDGE_ORIGIN),
                    ))
                    .execute(transaction.connection())
                    .map(|_| ())
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .unwrap_or_else(|error| panic!("Runtime Config fixture failed: {error:?}"));
        let state = Arc::new(
            server_state::tests::for_test(database.clone())
                .unwrap_or_else(|error| panic!("ServerState fixture failed: {error}")),
        );
        Self {
            path,
            database,
            state,
        }
    }

    async fn activate(&self, signing_key: &SigningKey) -> DeviceId {
        self.state.provisioning().open_window().await;
        let evidence = ValidatedEnrollmentEvidence::new(
            MachineHardwareId::parse(MACHINE_HARDWARE_ID)
                .unwrap_or_else(|| panic!("the fixture Machine Hardware ID is valid")),
            ControlPublicKey::parse(&signing_key.verifying_key().to_bytes())
                .unwrap_or_else(|| panic!("the fixture control key is valid")),
            EvidenceQuality::Strong,
            "2.0.0".to_owned(),
            "2.0.0".to_owned(),
        );
        let (review, activation) = match self
            .state
            .device()
            .start_enrollment(self.state.provisioning(), evidence)
            .await
            .unwrap_or_else(|error| panic!("Enrollment start failed: {error:?}"))
        {
            EnrollmentStartOutcome::Pending(review, activation) => (review, activation),
            EnrollmentStartOutcome::Replay(_) => panic!("a new fixture authority was replayed"),
        };
        let authority = self
            .state
            .device()
            .approve_enrollment(self.state.provisioning(), review.review_id())
            .await
            .unwrap_or_else(|error| panic!("Enrollment approval failed: {error:?}"));
        assert_eq!(activation.await, Ok(Ok(authority)));
        authority.device_id()
    }

    async fn component_row_counts(&self) -> [i64; 4] {
        self.database
            .read(|transaction| {
                Ok::<_, diesel::result::Error>([
                    gateway_credentials::table
                        .count()
                        .get_result(transaction.connection())?,
                    binding_negotiations::table
                        .count()
                        .get_result(transaction.connection())?,
                    device_session_targets::table
                        .count()
                        .get_result(transaction.connection())?,
                    device_home_targets::table
                        .count()
                        .get_result(transaction.connection())?,
                ])
            })
            .await
            .unwrap_or_else(|error| panic!("component row count failed: {error:?}"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for path in [
            self.path.clone(),
            PathBuf::from(format!("{}-wal", self.path.display())),
            PathBuf::from(format!("{}-shm", self.path.display())),
        ] {
            let _ = fs::remove_file(path);
        }
    }
}
