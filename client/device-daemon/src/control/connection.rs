use std::{fs, future::pending, sync::Arc};

use futures_util::{SinkExt as _, StreamExt as _};
use natsume_device_protocol::{
    CONTROL_ROUTE, CONTROL_SUBPROTOCOL,
    generated::{
        ClientActiveEnvelope, ClientStateSnapshot, EnrollmentEvidenceQuality, ServerActiveEnvelope,
        ServerStateSnapshot, client_active_envelope, server_active_envelope,
    },
};
use prost::Message as _;
use rustls::{ClientConfig, RootCertStore};
use rustls_pki_types::{CertificateDer, pem::PemObject as _};
use serde::Deserialize;
use tokio::{
    net::TcpStream,
    task::JoinHandle,
    time::{Duration, Instant, Interval, MissedTickBehavior, timeout},
};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{client::IntoClientRequest as _, protocol::WebSocketConfig},
};

use crate::{
    CanonicalEndpoint,
    reconcile::{
        ReconcileOutcome, SnapshotError, SnapshotReconciler, ValidatedSnapshot,
        validate_server_snapshot,
    },
};

use super::{ControlIdentity, ControlLoopError, enrollment::HandshakeOutcome};

pub(super) const MAX_MESSAGE_BYTES: usize = 65_536;
const CLIENT_CONFIG_PATH: &str = "/etc/natsume/config.toml";
const CONTROL_ROOT_PATH: &str = "/etc/natsume/trust/control-ca.crt";
const INITIAL_RECONNECT_WINDOW_MS: u32 = 5_000;
const MAX_RECONNECT_WINDOW_MS: u32 = 30_000;
const STABLE_ACTIVE_DURATION: Duration = Duration::from_mins(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
pub(super) const SERVER_SILENCE_TIMEOUT: Duration = Duration::from_mins(1);
pub(super) const SEND_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type LocalSnapshotTask = JoinHandle<Result<ClientStateSnapshot, SnapshotError>>;

/// Immutable endpoint and pinned TLS material shared by reconnect attempts.
struct ConnectionSettings {
    endpoint: CanonicalEndpoint,
    tls: Arc<ClientConfig>,
    trust_fingerprint: String,
}

/// Retained by the connection loop across transport, handshake and short Active failures.
struct ReconnectBackoff {
    window_ms: u32,
}

impl ReconnectBackoff {
    fn new() -> Self {
        Self {
            window_ms: INITIAL_RECONNECT_WINDOW_MS,
        }
    }

    fn delay(&self, sample: u32) -> Duration {
        Duration::from_millis(u64::from(sample % (self.window_ms + 1)))
    }

    async fn wait(&self) -> Result<(), ControlLoopError> {
        let sample = getrandom::u32().map_err(|_| ControlLoopError::ReconnectEntropy)?;
        tokio::time::sleep(self.delay(sample)).await;
        Ok(())
    }

    fn advance(&mut self) {
        self.window_ms = (self.window_ms * 2).min(MAX_RECONNECT_WINDOW_MS);
    }
}

#[derive(Deserialize)]
struct ProductionConfig {
    server: CanonicalEndpoint,
}

impl ConnectionSettings {
    fn production() -> Result<Self, ControlLoopError> {
        let encoded = fs::read_to_string(CLIENT_CONFIG_PATH)
            .map_err(|_| ControlLoopError::EndpointConfiguration)?;
        let config = toml::from_str::<ProductionConfig>(&encoded)
            .map_err(|_| ControlLoopError::EndpointConfiguration)?;
        let endpoint = config.server;

        let encoded =
            fs::read(CONTROL_ROOT_PATH).map_err(|_| ControlLoopError::TrustRootConfiguration)?;
        let mut certificates = CertificateDer::pem_slice_iter(&encoded);
        let trust_root = certificates
            .next()
            .ok_or(ControlLoopError::TrustRootConfiguration)?
            .map_err(|_| ControlLoopError::TrustRootConfiguration)?;
        if certificates.next().is_some() {
            return Err(ControlLoopError::TrustRootConfiguration);
        }
        let trust_fingerprint = {
            use sha2::Digest as _;
            hex::encode(sha2::Sha256::digest(trust_root.as_ref()))
        };
        let mut roots = RootCertStore::empty();
        roots
            .add(trust_root)
            .map_err(|_| ControlLoopError::TrustRootConfiguration)?;
        let mut tls =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|_| ControlLoopError::Tls)?
                .with_root_certificates(roots)
                .with_no_client_auth();
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            endpoint,
            tls: Arc::new(tls),
            trust_fingerprint,
        })
    }

    fn origin(&self) -> String {
        control_url(self.endpoint)
            .trim_end_matches(CONTROL_ROUTE)
            .replacen("wss://", "https://", 1)
    }

    fn presentation_scope(&self, identity: &ControlIdentity) -> Option<String> {
        use sha2::Digest as _;
        let device = identity.manifest.device_id()?;
        Some(hex::encode(sha2::Sha256::digest(format!(
            "{}|{}|{}|{}",
            self.origin(),
            self.trust_fingerprint,
            device,
            hex::encode(identity.key.public_key())
        ))))
    }

    async fn connect(&self) -> Option<Socket> {
        let mut request = control_url(self.endpoint).into_client_request().ok()?;
        request
            .headers_mut()
            .insert("sec-websocket-protocol", CONTROL_SUBPROTOCOL.parse().ok()?);
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_MESSAGE_BYTES));
        let (socket, response) = timeout(
            CONNECT_TIMEOUT,
            connect_async_tls_with_config(
                request,
                Some(config),
                false,
                Some(Connector::Rustls(Arc::clone(&self.tls))),
            ),
        )
        .await
        .ok()?
        .ok()?;
        (response
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok())
            == Some(CONTROL_SUBPROTOCOL))
        .then_some(socket)
    }
}

pub(crate) async fn run(
    mut identity: ControlIdentity,
    machine_hardware_id: uuid::Uuid,
    evidence_quality: EnrollmentEvidenceQuality,
    snapshots: SnapshotReconciler,
) -> Result<(), ControlLoopError> {
    let snapshots = Arc::new(snapshots);
    snapshots
        .deactivate()
        .await
        .map_err(|_| ControlLoopError::LocalDeactivation)?;
    let settings = ConnectionSettings::production()?;
    if let Some(scope) = settings.presentation_scope(&identity) {
        snapshots
            .configure_presentation(scope)
            .map_err(|_| ControlLoopError::PresentationCache)?;
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .use_preconfigured_tls((*settings.tls).clone())
        .build()
        .map_err(|_| ControlLoopError::Tls)?;
    let _logos = tokio_util::task::AbortOnDropHandle::new(
        snapshots.start_logo_downloads(client, settings.origin()),
    );
    let mut backoff = ReconnectBackoff::new();
    let mut first_attempt = true;
    loop {
        // One tracked local pass runs alongside backoff/connect/handshake. It
        // owns only already-persisted work and transfers to the Active pump,
        // where a received Target waits for it before starting new effects.
        let recovery = Arc::clone(&snapshots);
        let local = tokio::spawn(async move { recovery.recover_local().await });
        let attempt = async {
            backoff.wait().await?;
            if !first_attempt {
                backoff.advance();
            }
            let Some(mut socket) = settings.connect().await else {
                tracing::warn!(
                    expected_subprotocol = CONTROL_SUBPROTOCOL,
                    "Device control connection failed"
                );
                return Ok(None);
            };
            match super::enrollment::handshake(
                &mut socket,
                &mut identity,
                machine_hardware_id,
                evidence_quality,
            )
            .await?
            {
                HandshakeOutcome::Active(session_id) => Ok(Some((socket, session_id))),
                HandshakeOutcome::Retry => Ok(None),
            }
        }
        .await;
        first_attempt = false;
        match attempt {
            Ok(Some((socket, session_id))) => {
                let scope = settings
                    .presentation_scope(&identity)
                    .ok_or(ControlLoopError::PresentationCache)?;
                snapshots
                    .configure_presentation(scope)
                    .map_err(|_| ControlLoopError::PresentationCache)?;
                let stable =
                    run_active(socket, session_id, Arc::clone(&snapshots), Some(local)).await;
                snapshots
                    .deactivate()
                    .await
                    .map_err(|_| ControlLoopError::LocalDeactivation)?;
                if stable {
                    backoff = ReconnectBackoff::new();
                }
            }
            result => {
                match local.await {
                    Ok(Ok(_)) => {}
                    Ok(Err(SnapshotError::Caddy)) => {
                        return Err(ControlLoopError::LocalDeactivation);
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(error = %error, "Local maintenance will retry");
                    }
                    Err(_) => tracing::error!("Local maintenance task terminated unexpectedly"),
                }
                result?;
            }
        }
    }
}

/// The only target plan eligible to publish a result for the current lease.
///
/// Replacement closes this cooperative fence and waits for the task before the
/// next plan starts. In-flight external operations use their own fixed deadlines;
/// the fence only prevents the next resource effect from starting.
struct CurrentPlan {
    fence: tokio_util::sync::CancellationToken,
    /// Retained validated target used to coalesce exact repeats without copying secrets.
    target: Arc<ValidatedSnapshot>,
    task: JoinHandle<Result<ReconcileOutcome<ClientStateSnapshot>, SnapshotError>>,
}

/// Latest validated target waiting for the canceled running plan to stop.
struct PendingPlan {
    fence: tokio_util::sync::CancellationToken,
    target: Arc<ValidatedSnapshot>,
}

/// Retry authority belongs to the last completed plan in this active lease.
struct RetrySchedule {
    target: Option<Arc<ValidatedSnapshot>>,
    deadline: Option<Instant>,
    delay_ms: u32,
}

impl RetrySchedule {
    fn new() -> Self {
        Self {
            target: None,
            deadline: None,
            delay_ms: 1_000,
        }
    }

    fn completed(&mut self, target: Arc<ValidatedSnapshot>, retry: bool) {
        self.target = Some(target);
        self.deadline = if retry {
            let jitter = getrandom::u32().unwrap_or(u32::MAX);
            let millis = self.delay_ms / 2 + jitter % (self.delay_ms / 2 + 1);
            self.delay_ms = (self.delay_ms * 2).min(30_000);
            Some(Instant::now() + Duration::from_millis(u64::from(millis)))
        } else {
            self.delay_ms = 1_000;
            None
        };
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the active pump keeps deadline and single local-work scheduling visible"
)]
async fn run_active(
    mut socket: Socket,
    session_id: [u8; 16],
    snapshots: Arc<SnapshotReconciler>,
    initial_local_task: Option<LocalSnapshotTask>,
) -> bool {
    let stable_at = Instant::now() + STABLE_ACTIVE_DURATION;
    let mut stable = false;
    let mut last_sent = None::<ClientStateSnapshot>;
    let (mut current, mut queued) = (None::<CurrentPlan>, None::<PendingPlan>);
    let mut observation =
        Some(initial_local_task.unwrap_or_else(|| start_observation(Arc::clone(&snapshots), None)));
    let mut retry = RetrySchedule::new();
    let mut local_change_pending = false;
    let mut awaiting_target = false;
    let mut snapshot_error = None::<SnapshotError>;
    let mut failed_task = None::<&'static str>;
    let mut maintenance = heartbeat_interval();
    let mut server_deadline = Instant::now() + SERVER_SILENCE_TIMEOUT;
    loop {
        let idle = current.is_none() && queued.is_none() && observation.is_none();
        let plan_finished = async {
            match current.as_mut() {
                Some(plan) => (&mut plan.task).await,
                None => pending().await,
            }
        };
        let observation_finished = async {
            match observation.as_mut() {
                Some(task) => task.await,
                None => pending().await,
            }
        };
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(server_deadline) => {
                break;
            }
            () = async {
                match retry.deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => pending().await,
                }
            }, if idle => {
                retry.deadline = None;
                let Some(target) = retry.target.as_ref() else { break; };
                let fence = tokio_util::sync::CancellationToken::new();
                if let Err(error) = snapshots.refresh_plan(&fence) {
                    snapshot_error = Some(error);
                    break;
                }
                current = Some(start_plan(Arc::clone(&snapshots), PendingPlan { fence, target: Arc::clone(target) }));
            }
            message = socket.next() => {
                let received_at = Instant::now();
                match decode_active(message, session_id) {
                    ActiveInput::Target(target) => {
                        if let Err(error) = queue_latest_plan(
                            &snapshots,
                            &mut current,
                            &mut queued,
                            &mut retry,
                            awaiting_target,
                            observation.is_some(),
                            *target,
                        )
                        {
                            snapshot_error = Some(error);
                            break;
                        }
                        awaiting_target = false;
                    }
                    ActiveInput::Alive => {}
                    ActiveInput::Retry => {
                        break;
                    }
                }
                server_deadline = received_at + SERVER_SILENCE_TIMEOUT;
                // Only valid traffic after the stability threshold earns a reset.
                stable |= received_at >= stable_at;
            }
            result = plan_finished => {
                let Some(finished) = current.take() else {
                    break;
                };
                if let Some(next) = queued.take() {
                    current = Some(start_plan(Arc::clone(&snapshots), next));
                    continue;
                }
                let outcome = match result {
                    Ok(Ok(outcome)) => outcome,
                    Ok(Err(error)) => {
                        snapshot_error = Some(error);
                        break;
                    }
                    Err(_) => {
                        failed_task = Some("reconciliation");
                        break;
                    }
                };
                retry.completed(finished.target, outcome.retry);
                if local_change_pending {
                    local_change_pending = false;
                    observation = Some(start_observation(Arc::clone(&snapshots), retry.target.clone()));
                    continue;
                }
                let Ok(sent) = send_changed_snapshot(&mut socket, session_id, &mut last_sent, outcome.actual).await else {
                    break;
                };
                awaiting_target |= sent;
            }
            result = observation_finished => {
                observation = None;
                if let Some(next) = queued.take() {
                    current = Some(start_plan(Arc::clone(&snapshots), next));
                    continue;
                }
                let snapshot = match result {
                    Ok(Ok(snapshot)) => snapshot,
                    Ok(Err(error)) => {
                        snapshot_error = Some(error);
                        break;
                    }
                    Err(_) => {
                        failed_task = Some("observation");
                        break;
                    }
                };
                if local_change_pending {
                    local_change_pending = false;
                    observation = Some(start_observation(Arc::clone(&snapshots), retry.target.clone()));
                    continue;
                }
                let Ok(sent) = send_changed_snapshot(&mut socket, session_id, &mut last_sent, snapshot).await else {
                    break;
                };
                awaiting_target |= sent;
            }
            heartbeat_due = async {
                tokio::select! {
                    () = snapshots.changed() => false,
                    _ = maintenance.tick() => true,
                }
            } => {
                if heartbeat_due && !send_heartbeat(&mut socket, session_id).await {
                    break;
                }
                if current.is_some() {
                    local_change_pending = true;
                } else if observation.is_some() {
                    local_change_pending |= !heartbeat_due;
                } else {
                    observation = Some(start_observation(Arc::clone(&snapshots), retry.target.clone()));
                }
            }
        }
    }
    snapshots.presentation_offline();
    drop(socket);
    fence_plans(&snapshots, current, queued).await;
    if let Some(task) = observation {
        // The initial task can own a local Home/termination recovery. Closing
        // WSS does not cancel a Helper mutation; join it before reconnecting.
        let _ = task.await;
    }
    if let Some(error) = snapshot_error {
        tracing::error!(error = %error, "Device active snapshot processing failed");
    } else if let Some(task) = failed_task {
        tracing::error!(task, "Device active snapshot task terminated unexpectedly");
    }
    stable
}

fn queue_latest_plan(
    snapshots: &Arc<SnapshotReconciler>,
    current: &mut Option<CurrentPlan>,
    pending: &mut Option<PendingPlan>,
    retry: &mut RetrySchedule,
    awaiting_target: bool,
    observation_running: bool,
    target: ServerStateSnapshot,
) -> Result<(), SnapshotError> {
    let presentation = crate::reconcile::snapshot_presentation(&target);
    let target = Arc::new(validate_server_snapshot(target)?);
    snapshots.accept_presentation(presentation)?;
    if target_is_redundant(
        current.as_ref().map(|plan| plan.target.as_ref()),
        pending.as_ref().map(|plan| plan.target.as_ref()),
        retry.target.as_deref(),
        awaiting_target && retry.deadline.is_none(),
        target.as_ref(),
    ) {
        return Ok(());
    }
    let unchanged = retry.target.as_deref() == Some(target.as_ref());
    *retry = RetrySchedule::new();
    let fence = tokio_util::sync::CancellationToken::new();
    if unchanged {
        snapshots.refresh_plan(&fence)?;
    } else {
        snapshots.begin_plan(&fence)?;
    }
    let latest = PendingPlan { fence, target };
    if current.is_some() || observation_running {
        if let Some(current) = current.as_ref() {
            current.fence.cancel();
        }
        if let Some(replaced) = pending.replace(latest) {
            replaced.fence.cancel();
        }
    } else {
        *current = Some(start_plan(Arc::clone(snapshots), latest));
    }
    Ok(())
}

fn target_is_redundant(
    current: Option<&ValidatedSnapshot>,
    queued: Option<&ValidatedSnapshot>,
    previous: Option<&ValidatedSnapshot>,
    awaiting_target: bool,
    incoming: &ValidatedSnapshot,
) -> bool {
    queued.or(current).is_some_and(|target| target == incoming)
        || (queued.is_none() && current.is_none() && !awaiting_target && previous == Some(incoming))
}

async fn send_changed_snapshot(
    socket: &mut Socket,
    session_id: [u8; 16],
    last_sent: &mut Option<ClientStateSnapshot>,
    snapshot: ClientStateSnapshot,
) -> Result<bool, ()> {
    if last_sent.as_ref() == Some(&snapshot) {
        return Ok(false);
    }
    if !send_snapshot(socket, session_id, &snapshot).await {
        return Err(());
    }
    *last_sent = Some(snapshot);
    Ok(true)
}

pub(super) fn heartbeat_interval() -> Interval {
    let mut interval =
        tokio::time::interval_at(Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    interval
}

fn start_plan(snapshots: Arc<SnapshotReconciler>, pending: PendingPlan) -> CurrentPlan {
    let PendingPlan { fence, target } = pending;
    let task_fence = fence.clone();
    let task_target = Arc::clone(&target);
    let task = tokio::spawn(async move { snapshots.reconcile(&task_target, task_fence).await });
    CurrentPlan {
        fence,
        target,
        task,
    }
}

fn start_observation(
    snapshots: Arc<SnapshotReconciler>,
    target: Option<Arc<ValidatedSnapshot>>,
) -> LocalSnapshotTask {
    tokio::spawn(async move { snapshots.observe(target.as_deref()).await })
}

async fn fence_plans(
    snapshots: &SnapshotReconciler,
    current: Option<CurrentPlan>,
    pending: Option<PendingPlan>,
) {
    if snapshots.end_plan().is_err() {
        tracing::error!("Binding plan authority could not be revoked");
    }
    if let Some(pending) = pending {
        pending.fence.cancel();
    }
    if let Some(plan) = current {
        plan.fence.cancel();
        let _ = plan.task.await;
    }
}

enum ActiveInput {
    Target(Box<ServerStateSnapshot>),
    Alive,
    Retry,
}

fn decode_active(
    message: Option<
        Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>,
    >,
    session_id: [u8; 16],
) -> ActiveInput {
    let Some(Ok(message)) = message else {
        return ActiveInput::Retry;
    };
    if let tokio_tungstenite::tungstenite::Message::Pong(payload) = &message {
        return if payload.as_ref() == session_id.as_slice() {
            ActiveInput::Alive
        } else {
            ActiveInput::Retry
        };
    }
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
        return ActiveInput::Retry;
    };
    if bytes.len() > MAX_MESSAGE_BYTES {
        return ActiveInput::Retry;
    }
    let Ok(envelope) = ServerActiveEnvelope::decode(bytes) else {
        return ActiveInput::Retry;
    };
    if envelope.session_id.as_slice() != session_id {
        return ActiveInput::Retry;
    }
    match envelope.body {
        Some(server_active_envelope::Body::ServerState(target)) => {
            ActiveInput::Target(Box::new(target))
        }
        None => ActiveInput::Retry,
    }
}

async fn send_heartbeat(socket: &mut Socket, session_id: [u8; 16]) -> bool {
    matches!(
        timeout(
            SEND_TIMEOUT,
            socket.send(tokio_tungstenite::tungstenite::Message::Ping(
                session_id.to_vec().into(),
            )),
        )
        .await,
        Ok(Ok(()))
    )
}

async fn send_snapshot(
    socket: &mut Socket,
    session_id: [u8; 16],
    snapshot: &ClientStateSnapshot,
) -> bool {
    let envelope = ClientActiveEnvelope {
        session_id: session_id.to_vec(),
        body: Some(client_active_envelope::Body::ClientState(snapshot.clone())),
    };
    let bytes = envelope.encode_to_vec();
    bytes.len() <= MAX_MESSAGE_BYTES
        && matches!(
            timeout(
                SEND_TIMEOUT,
                socket.send(tokio_tungstenite::tungstenite::Message::Binary(
                    bytes.into(),
                )),
            )
            .await,
            Ok(Ok(()))
        )
}

fn control_url(endpoint: CanonicalEndpoint) -> String {
    match endpoint.ip {
        std::net::IpAddr::V4(ip) => {
            format!("wss://{ip}:{}{CONTROL_ROUTE}", endpoint.port)
        }
        std::net::IpAddr::V6(ip) => {
            format!("wss://[{ip}]:{}{CONTROL_ROUTE}", endpoint.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use natsume_device_protocol::generated::{
        BindingAccessTarget, BindingNegotiationIntent, ConcreteTargetState, ForegroundTarget,
        GatewayCredentialIntent, GatewayTarget, HomeTarget, RuntimeConfigTarget, ServerIntentState,
        SessionControlTarget,
    };
    use std::os::unix::fs::MetadataExt as _;
    use uuid::Uuid;

    use super::*;

    use crate::reconcile::tests::{Fixture, HelperState, fixture, snapshot};
    use tokio_tungstenite::tungstenite::{Message as WsMessage, protocol::Role};

    struct ActiveFixture {
        resources: Fixture,
        socket: WebSocketStream<TcpStream>,
        pump: JoinHandle<bool>,
        session_id: [u8; 16],
    }

    impl Drop for ActiveFixture {
        fn drop(&mut self) {
            self.pump.abort();
        }
    }

    impl ActiveFixture {
        async fn target(
            &mut self,
            target: ServerStateSnapshot,
        ) -> Result<(), Box<dyn std::error::Error>> {
            self.socket
                .send(WsMessage::Binary(
                    ServerActiveEnvelope {
                        session_id: self.session_id.to_vec(),
                        body: Some(server_active_envelope::Body::ServerState(target)),
                    }
                    .encode_to_vec()
                    .into(),
                ))
                .await?;
            Ok(())
        }

        async fn next_snapshot(
            &mut self,
        ) -> Result<ClientStateSnapshot, Box<dyn std::error::Error>> {
            loop {
                let message = timeout(Duration::from_secs(10), self.socket.next())
                    .await?
                    .ok_or("control connection closed")??;
                match message {
                    WsMessage::Binary(bytes) => {
                        let envelope = ClientActiveEnvelope::decode(bytes.as_ref())?;
                        assert_eq!(envelope.session_id, self.session_id);
                        if let Some(client_active_envelope::Body::ClientState(snapshot)) =
                            envelope.body
                        {
                            return Ok(snapshot);
                        }
                    }
                    WsMessage::Ping(bytes) => self.socket.send(WsMessage::Pong(bytes)).await?,
                    _ => return Err("unexpected control message".into()),
                }
            }
        }
    }

    async fn active_fixture(
        state: HelperState,
    ) -> Result<ActiveFixture, Box<dyn std::error::Error>> {
        let resources = fixture(state).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let client = TcpStream::connect(listener.local_addr()?).await?;
        let (server, _) = listener.accept().await?;
        let client =
            WebSocketStream::from_raw_socket(MaybeTlsStream::Plain(client), Role::Client, None)
                .await;
        let socket = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        let session_id = *Uuid::now_v7().as_bytes();
        let pump = tokio::spawn(run_active(
            client,
            session_id,
            Arc::clone(&resources.snapshots),
            None,
        ));
        let mut fixture = ActiveFixture {
            resources,
            socket,
            pump,
            session_id,
        };
        fixture.next_snapshot().await?;
        Ok(fixture)
    }

    #[tokio::test]
    async fn local_safety_observation_continues_while_waiting_for_the_server()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut active = active_fixture(HelperState::default()).await?;
        active.target(snapshot()).await?;
        active.next_snapshot().await?;
        // Leave the ClientState unanswered. Only the local observation changes.
        active
            .resources
            .helper
            .lock()
            .map_err(|_| "fixture lock")?
            .session_state = Some(natsume_local_control_api::GraphicalSessionState::Ambiguous);
        tokio::time::pause();
        tokio::time::advance(HEARTBEAT_INTERVAL).await;
        tokio::time::resume();
        let actual = active
            .next_snapshot()
            .await?
            .actual
            .ok_or("missing actual")?;
        assert_eq!(
            actual
                .session_control
                .ok_or("missing Session")?
                .session_state,
            i32::from(natsume_device_protocol::generated::SessionState::Ambiguous)
        );
        assert!(!active.pump.is_finished());
        Ok(())
    }

    #[test]
    fn reconnect_windows_grow_to_a_cap_and_allow_the_full_delay_range() {
        let mut backoff = ReconnectBackoff::new();
        for window in [5_000, 10_000, 20_000, 30_000, 30_000] {
            let ceiling = Duration::from_millis(u64::from(window));
            assert_eq!(backoff.delay(0), Duration::ZERO);
            assert_eq!(backoff.delay(window), ceiling);
            assert!(backoff.delay(u32::MAX) <= ceiling);
            backoff.advance();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn initial_reconnect_wait_does_not_consume_the_first_retry_window()
    -> Result<(), ControlLoopError> {
        let backoff = ReconnectBackoff::new();
        let started = Instant::now();
        backoff.wait().await?;
        assert!(started.elapsed() <= Duration::from_secs(5));
        assert_eq!(backoff.window_ms, 5_000);
        Ok(())
    }

    #[test]
    fn reconnect_attempts_are_spread_across_a_600_device_outage()
    -> Result<(), Box<dyn std::error::Error>> {
        use sha2::{Digest as _, Sha256};

        let mut startup = [0_u32; 6];
        let mut attempts_per_second = [0_u32; 120];
        for device in 0..600 {
            let mut backoff = ReconnectBackoff::new();
            let mut elapsed = Duration::ZERO;
            for attempt in 0.. {
                // Fixed independent samples keep this distribution check reproducible.
                let digest = Sha256::digest(format!("{device}:{attempt}").as_bytes());
                let sample = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]);
                elapsed += backoff.delay(sample);
                if elapsed >= Duration::from_mins(2) {
                    break;
                }
                let second = usize::try_from(elapsed.as_secs())?;
                attempts_per_second[second] += 1;
                if attempt == 0 {
                    startup[second] += 1;
                } else {
                    backoff.advance();
                }
            }
        }
        assert_eq!(startup.iter().sum::<u32>(), 600);
        assert!(startup[..5].iter().all(|count| (60..180).contains(count)));
        let peak = attempts_per_second
            .into_iter()
            .max()
            .ok_or("empty timeline")?;
        // Fixed five-second waits put all 600 attempts in the same second.
        assert!(
            peak < 300,
            "reconnect attempts clustered: {attempts_per_second:?}"
        );
        println!("600-device simulation: startup per second = {startup:?}; peak = {peak}/s");
        Ok(())
    }

    #[tokio::test]
    async fn active_stability_requires_valid_traffic_after_sixty_seconds()
    -> Result<(), Box<dyn std::error::Error>> {
        for (later_seconds, ending, expected) in [
            (10, "pong", false),
            (26, "pong", true),
            (26, "target", true),
            (26, "stale_pong", false),
            (26, "invalid_target", false),
        ] {
            let mut active = active_fixture(HelperState::default()).await?;
            // Advance only between I/O exchanges so virtual time cannot outrun
            // the socket and Helper/Caddy fixture responses.
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(35)).await;
            tokio::time::resume();
            active.target(snapshot()).await?;
            active.next_snapshot().await?;

            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(later_seconds)).await;
            tokio::time::resume();
            let final_message = match ending {
                "pong" => WsMessage::Pong(active.session_id.to_vec().into()),
                "stale_pong" => WsMessage::Pong(vec![0; 16].into()),
                "target" | "invalid_target" => WsMessage::Binary(
                    ServerActiveEnvelope {
                        session_id: active.session_id.to_vec(),
                        body: Some(server_active_envelope::Body::ServerState(
                            if ending == "target" {
                                snapshot()
                            } else {
                                ServerStateSnapshot::default()
                            },
                        )),
                    }
                    .encode_to_vec()
                    .into(),
                ),
                _ => return Err("unknown test ending".into()),
            };
            active.socket.feed(final_message).await?;
            active.socket.feed(WsMessage::Close(None)).await?;
            active.socket.flush().await?;
            let stable = timeout(Duration::from_secs(10), &mut active.pump).await??;
            assert_eq!(
                stable,
                expected,
                "{ending} after {} seconds",
                35 + later_seconds
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_silent_active_session_does_not_earn_a_backoff_reset()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut active = active_fixture(HelperState::default()).await?;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::time::resume();
        assert!(!timeout(Duration::from_secs(10), &mut active.pump).await??);
        Ok(())
    }

    fn destructive_target() -> ServerStateSnapshot {
        let mut target = snapshot();
        let concrete = target
            .target
            .as_mut()
            .unwrap_or_else(|| panic!("fixture target"));
        concrete
            .session_control
            .as_mut()
            .unwrap_or_else(|| panic!("fixture session"))
            .terminate_epoch = Some(7);
        concrete
            .home
            .as_mut()
            .unwrap_or_else(|| panic!("fixture home"))
            .reset_epoch = Some(8);
        target
    }

    #[tokio::test]
    async fn unchanged_failures_retry_in_one_control_session_and_complete_exact_epochs()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut active = active_fixture(HelperState {
            terminate_failures: 2,
            home_failures: 2,
            ..HelperState::default()
        })
        .await?;
        let target = destructive_target();
        active.target(target.clone()).await?;
        // No more Server targets are sent, including after the unchanged second failure.
        loop {
            let snapshot = active.next_snapshot().await?;
            let actual = snapshot.actual.ok_or("missing actual")?;
            if actual
                .session_control
                .as_ref()
                .and_then(|s| s.completed_terminate_epoch)
                == Some(7)
                && actual.home.as_ref().and_then(|h| h.completed_reset_epoch) == Some(8)
            {
                break;
            }
        }
        {
            let state = active
                .resources
                .helper
                .lock()
                .unwrap_or_else(|e| panic!("fixture lock: {e}"));
            assert_eq!(state.termination_calls.len(), 3);
            assert!(
                state
                    .termination_calls
                    .iter()
                    .all(|s| s.logind_session_id == "c2")
            );
            assert_eq!(state.home_calls, [8, 8, 8]);
        }
        let session_path = active
            .resources
            .directory
            .path()
            .join("session-completion.json");
        let home_path = active
            .resources
            .directory
            .path()
            .join("home-completion.json");
        let session_before = fs::metadata(&session_path)?.ino();
        let home_before = fs::metadata(&home_path)?.ino();
        // A later replay of the completed target must not rewrite either completion.
        active.target(target).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(fs::metadata(session_path)?.ino(), session_before);
        assert_eq!(fs::metadata(home_path)?.ino(), home_before);
        assert!(!active.pump.is_finished());
        Ok(())
    }

    #[tokio::test]
    async fn rejected_new_operations_do_not_schedule_destructive_retries()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut active = active_fixture(HelperState {
            rejected: true,
            ..HelperState::default()
        })
        .await?;
        active.target(destructive_target()).await?;
        active.next_snapshot().await?;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let state = active
            .resources
            .helper
            .lock()
            .unwrap_or_else(|e| panic!("fixture lock: {e}"));
        assert_eq!(state.termination_calls.len(), 1);
        assert!(state.home_calls.is_empty());
        assert!(state.progress.is_none());
        assert!(
            !active
                .resources
                .directory
                .path()
                .join("home-completion.json")
                .exists()
        );
        Ok(())
    }

    #[tokio::test]
    async fn owned_home_rejection_retries_after_repair_without_a_new_target()
    -> Result<(), Box<dyn std::error::Error>> {
        use natsume_local_control_api::{HomeResetPhase, HomeResetProgress};

        let mut active = active_fixture(HelperState {
            rejected: true,
            progress: Some(HomeResetProgress {
                reset_epoch: 8,
                phase: HomeResetPhase::RecoveryRequired,
            }),
            ..HelperState::default()
        })
        .await?;
        let mut target = snapshot();
        target
            .target
            .as_mut()
            .ok_or("missing target")?
            .home
            .as_mut()
            .ok_or("missing Home")?
            .reset_epoch = Some(8);
        active.target(target).await?;
        active.next_snapshot().await?;
        // The same rejected mount remains observable. It must not clear the
        // retry schedule for the already-owned epoch, even without new Targets.
        timeout(Duration::from_secs(5), async {
            loop {
                let attempts = active
                    .resources
                    .helper
                    .lock()
                    .map_err(|_| "fixture lock")?
                    .home_calls
                    .len();
                if attempts >= 2 {
                    break Ok::<_, Box<dyn std::error::Error>>(());
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await??;
        let completed = active
            .resources
            .directory
            .path()
            .join("home-completion.json");
        assert!(!completed.exists());
        {
            let mut state = active.resources.helper.lock().map_err(|_| "fixture lock")?;
            // Rejected calls did not apply a mount. This fixture's verifier
            // must still wait for a successful recovery after the repair.
            state.home_failures = state.home_calls.len();
            state.rejected = false;
        }
        // Repair only the Helper's environment; no target replay or reconnect.
        timeout(Duration::from_secs(5), async {
            loop {
                let actual = active
                    .next_snapshot()
                    .await?
                    .actual
                    .ok_or("missing actual")?;
                if actual.home.and_then(|home| home.completed_reset_epoch) == Some(8) {
                    break Ok::<_, Box<dyn std::error::Error>>(());
                }
            }
        })
        .await??;
        let state = active.resources.helper.lock().map_err(|_| "fixture lock")?;
        assert!(state.home_calls.len() >= 3);
        assert!(state.home_calls.iter().all(|epoch| *epoch == 8));
        assert!(state.termination_calls.is_empty());
        assert!(completed.exists());
        assert!(!active.pump.is_finished());
        Ok(())
    }

    #[tokio::test]
    async fn replacement_target_finishes_owned_epochs_without_retargeting()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut active = active_fixture(HelperState {
            terminate_failures: 2,
            home_failures: 2,
            ..HelperState::default()
        })
        .await?;
        let mut target = destructive_target();
        active.target(target.clone()).await?;
        active.next_snapshot().await?;
        let concrete = target.target.as_mut().ok_or("missing target")?;
        concrete
            .session_control
            .as_mut()
            .ok_or("missing session")?
            .terminate_epoch = None;
        concrete.home.as_mut().ok_or("missing home")?.reset_epoch = None;
        active.target(target).await?;
        timeout(Duration::from_secs(5), async {
            loop {
                let actual = active
                    .next_snapshot()
                    .await?
                    .actual
                    .ok_or("missing actual")?;
                if actual
                    .session_control
                    .as_ref()
                    .is_some_and(|s| s.completed_terminate_epoch == Some(7))
                    && actual
                        .home
                        .as_ref()
                        .is_some_and(|h| h.completed_reset_epoch == Some(8))
                {
                    return Ok::<_, Box<dyn std::error::Error>>(());
                }
            }
        })
        .await??;
        let state = active
            .resources
            .helper
            .lock()
            .unwrap_or_else(|e| panic!("fixture lock: {e}"));
        assert_eq!(state.termination_calls.len(), 3);
        assert!(
            state
                .termination_calls
                .iter()
                .all(|session| session == &state.termination_calls[0])
        );
        assert_eq!(state.termination_calls[0].logind_session_id, "c2");
        assert_eq!(state.home_calls, [8, 8, 8]);
        Ok(())
    }

    #[tokio::test]
    async fn silence_closes_transport_but_joins_owned_recovery_before_reconnect()
    -> Result<(), Box<dyn std::error::Error>> {
        use natsume_local_control_api::{HomeResetPhase, HomeResetProgress};
        let resources = fixture(HelperState {
            progress: Some(HomeResetProgress {
                reset_epoch: 8,
                phase: HomeResetPhase::Prepared,
            }),
            ..HelperState::default()
        })
        .await?;
        let release = Arc::new(tokio::sync::Notify::new());
        let wake = Arc::clone(&release);
        let recovery = Arc::clone(&resources.snapshots);
        let local = tokio::spawn(async move {
            wake.notified().await;
            recovery.recover_local().await
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let client = TcpStream::connect(listener.local_addr()?).await?;
        let (server, _) = listener.accept().await?;
        let client =
            WebSocketStream::from_raw_socket(MaybeTlsStream::Plain(client), Role::Client, None)
                .await;
        let socket = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        let session_id = *Uuid::now_v7().as_bytes();
        let pump = tokio::spawn(run_active(
            client,
            session_id,
            Arc::clone(&resources.snapshots),
            Some(local),
        ));
        let mut active = ActiveFixture {
            resources,
            socket,
            pump,
            session_id,
        };
        let mut newer = destructive_target();
        newer
            .target
            .as_mut()
            .ok_or("missing target")?
            .home
            .as_mut()
            .ok_or("missing home")?
            .reset_epoch = Some(9);
        active.target(newer).await?;
        active.resources.wait_for_plan().await?;
        tokio::time::pause();
        tokio::time::advance(HEARTBEAT_INTERVAL + Duration::from_secs(1)).await;
        tokio::time::resume();
        let heartbeat = timeout(Duration::from_secs(2), active.socket.next())
            .await
            .map_err(|_| "local recovery blocked the heartbeat")?
            .ok_or("missing heartbeat")??;
        assert!(matches!(heartbeat, WsMessage::Ping(_)));
        tokio::time::pause();
        tokio::time::advance(SERVER_SILENCE_TIMEOUT + Duration::from_secs(1)).await;
        tokio::time::resume();
        timeout(Duration::from_secs(2), async {
            while matches!(active.socket.next().await, Some(Ok(_))) {}
        })
        .await
        .map_err(|_| "local recovery blocked transport shutdown")?;
        assert!(!active.pump.is_finished(), "owned recovery must be joined");
        assert!(
            active
                .resources
                .helper
                .lock()
                .map_err(|_| "fixture lock")?
                .home_calls
                .is_empty()
        );
        release.notify_one();
        timeout(Duration::from_secs(5), &mut active.pump)
            .await
            .map_err(|_| "owned recovery did not finish after release")??;
        let state = active.resources.helper.lock().map_err(|_| "fixture lock")?;
        assert_eq!(state.home_calls, [8]);
        assert!(
            state.termination_calls.is_empty(),
            "queued Target must stay fenced"
        );
        let completion: serde_json::Value = serde_json::from_slice(&fs::read(
            active
                .resources
                .directory
                .path()
                .join("home-completion.json"),
        )?)?;
        assert_eq!(completion["completed_reset_epoch"], 8);
        Ok(())
    }

    #[tokio::test]
    async fn duplicate_target_does_not_reset_retry_even_when_awaiting_reply()
    -> Result<(), Box<dyn std::error::Error>> {
        let resources = fixture(HelperState::default()).await?;
        let wire = snapshot();
        let target = Arc::new(validate_server_snapshot(wire.clone())?);
        let mut retry = RetrySchedule::new();
        retry.completed(target, true);
        let deadline = retry.deadline;
        let delay = retry.delay_ms;
        let (mut current, mut queued) = (None, None);
        for _ in 0..20 {
            queue_latest_plan(
                &resources.snapshots,
                &mut current,
                &mut queued,
                &mut retry,
                true,
                false,
                wire.clone(),
            )?;
        }
        assert_eq!(retry.deadline, deadline);
        assert_eq!(retry.delay_ms, delay);
        assert!(current.is_none() && queued.is_none());
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn retry_delay_is_bounded_and_cleared_when_local_work_finishes() {
        let target = Arc::new(target(ForegroundTarget::Contest));
        let mut retry = RetrySchedule::new();
        for ceiling in [1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000] {
            let now = Instant::now();
            retry.completed(Arc::clone(&target), true);
            let delay = retry.deadline.unwrap_or_else(|| panic!("retry deadline")) - now;
            assert!(delay >= Duration::from_millis(ceiling / 2));
            assert!(delay <= Duration::from_millis(ceiling));
        }
        retry.completed(target, false);
        assert!(retry.deadline.is_none());
        assert_eq!(retry.delay_ms, 1_000);
    }

    #[test]
    fn deployment_config_reads_server_section_and_rejects_invalid_endpoints() {
        let example = include_str!("../../../../packaging/client/config.example.toml");
        let config: ProductionConfig = toml::from_str(example)
            .unwrap_or_else(|error| panic!("deployment example must parse: {error}"));
        assert_eq!(config.server.ip.to_string(), "192.0.2.10");
        assert_eq!(config.server.port.get(), 8443);
        for invalid in [
            example.replace("192.0.2.10", "server.example"),
            example.replace("192.0.2.10", "[2001:db8::1]"),
            example.replace("port = 8443", "port = 0"),
            example.replace("port = 8443", "port = 65536"),
            example.replace("[server]", "[unrelated]"),
        ] {
            assert!(toml::from_str::<ProductionConfig>(&invalid).is_err());
        }
    }

    #[test]
    fn control_url_uses_only_the_fixed_route_and_configured_ip_endpoint() {
        let ipv4 = toml::from_str::<CanonicalEndpoint>("ip = \"192.0.2.10\"\nport = 8443")
            .unwrap_or_else(|error| panic!("IPv4 fixture must parse: {error}"));
        assert_eq!(
            control_url(ipv4),
            "wss://192.0.2.10:8443/api/v2/device/control"
        );

        let ipv6 = toml::from_str::<CanonicalEndpoint>("ip = \"2001:db8::1\"\nport = 443")
            .unwrap_or_else(|error| panic!("IPv6 fixture must parse: {error}"));
        assert_eq!(
            control_url(ipv6),
            "wss://[2001:db8::1]:443/api/v2/device/control"
        );
    }

    #[test]
    fn active_envelopes_are_fenced_by_the_exact_session() {
        let session_id = *Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001).as_bytes();
        let envelope = ServerActiveEnvelope {
            session_id: session_id.to_vec(),
            body: Some(server_active_envelope::Body::ServerState(
                ServerStateSnapshot::default(),
            )),
        };
        let message = Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(
            envelope.encode_to_vec().into(),
        )));
        assert!(matches!(
            decode_active(message, session_id),
            ActiveInput::Target(_)
        ));

        let stale_session = *Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0002).as_bytes();
        let message = Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(
            envelope.encode_to_vec().into(),
        )));
        assert!(matches!(
            decode_active(message, stale_session),
            ActiveInput::Retry
        ));
    }

    #[test]
    fn active_heartbeat_is_fenced_by_the_exact_session() {
        let session_id = *Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001).as_bytes();
        let matching = Some(Ok(tokio_tungstenite::tungstenite::Message::Pong(
            session_id.to_vec().into(),
        )));
        assert!(matches!(
            decode_active(matching, session_id),
            ActiveInput::Alive
        ));

        let stale_session = *Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0002).as_bytes();
        let stale = Some(Ok(tokio_tungstenite::tungstenite::Message::Pong(
            stale_session.to_vec().into(),
        )));
        assert!(matches!(
            decode_active(stale, session_id),
            ActiveInput::Retry
        ));
    }

    #[test]
    fn periodic_repeats_are_coalesced_before_plan_authority_changes() {
        let current = target(ForegroundTarget::Contest);
        let queued = target(ForegroundTarget::Waiting);

        assert!(target_is_redundant(
            Some(&current),
            None,
            None,
            false,
            &current,
        ));
        assert!(target_is_redundant(
            Some(&current),
            Some(&queued),
            None,
            false,
            &queued,
        ));
        assert!(target_is_redundant(
            None,
            None,
            Some(&current),
            false,
            &current,
        ));
        assert!(!target_is_redundant(
            Some(&current),
            Some(&queued),
            None,
            false,
            &current,
        ));
    }

    #[test]
    fn changed_actual_allows_the_same_idle_target_to_run_again() {
        let previous = target(ForegroundTarget::Contest);

        assert!(!target_is_redundant(
            None,
            None,
            Some(&previous),
            true,
            &previous,
        ));
    }

    fn target(foreground_target: ForegroundTarget) -> ValidatedSnapshot {
        let credential_id = "01900000-0000-7000-8000-000000000001".to_owned();
        validate_server_snapshot(ServerStateSnapshot {
            intent: Some(ServerIntentState {
                gateway_credential: Some(GatewayCredentialIntent {
                    credential_id: credential_id.clone(),
                }),
                binding: Some(BindingNegotiationIntent {
                    negotiation_id: "01900000-0000-7000-8000-000000000002".to_owned(),
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
                    foreground_target: foreground_target.into(),
                    terminate_epoch: None,
                }),
                home: Some(HomeTarget { reset_epoch: None }),
            }),
        })
        .unwrap_or_else(|error| panic!("test target must validate: {error}"))
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_interval_has_no_immediate_tick() {
        let mut interval = heartbeat_interval();
        assert!(
            tokio::time::timeout(
                HEARTBEAT_INTERVAL.saturating_sub(Duration::from_millis(1)),
                interval.tick(),
            )
            .await
            .is_err()
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        interval.tick().await;
    }
}
