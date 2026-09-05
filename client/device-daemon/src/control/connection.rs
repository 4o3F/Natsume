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
    reconcile::{SnapshotError, SnapshotReconciler, ValidatedSnapshot, validate_server_snapshot},
};

use super::{ControlIdentity, ControlLoopError, enrollment::HandshakeOutcome};

pub(super) const MAX_MESSAGE_BYTES: usize = 65_536;
const CLIENT_CONFIG_PATH: &str = "/etc/natsume/config.toml";
const CONTROL_ROOT_PATH: &str = "/etc/natsume/trust/control-ca.crt";
const RECONNECT_DELAY: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
pub(super) const SERVER_SILENCE_TIMEOUT: Duration = Duration::from_mins(1);
pub(super) const SEND_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Immutable endpoint and pinned TLS material shared by reconnect attempts.
struct ConnectionSettings {
    endpoint: CanonicalEndpoint,
    tls: Arc<ClientConfig>,
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
        })
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
    loop {
        let Some(mut socket) = settings.connect().await else {
            tracing::warn!("Device control connection failed");
            reconnect_delay().await;
            continue;
        };
        let outcome = super::enrollment::handshake(
            &mut socket,
            &mut identity,
            machine_hardware_id,
            evidence_quality,
        )
        .await?;
        match outcome {
            HandshakeOutcome::Active(session_id) => {
                run_active(socket, session_id, Arc::clone(&snapshots)).await;
                snapshots
                    .deactivate()
                    .await
                    .map_err(|_| ControlLoopError::LocalDeactivation)?;
            }
            HandshakeOutcome::Retry => {}
        }
        reconnect_delay().await;
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
    task: JoinHandle<Result<ClientStateSnapshot, SnapshotError>>,
}

/// Latest validated target waiting for the canceled running plan to stop.
struct PendingPlan {
    fence: tokio_util::sync::CancellationToken,
    target: Arc<ValidatedSnapshot>,
}

#[expect(
    clippy::too_many_lines,
    reason = "the active pump keeps deadline and single local-work scheduling visible"
)]
async fn run_active(mut socket: Socket, session_id: [u8; 16], snapshots: Arc<SnapshotReconciler>) {
    let mut last_sent = None::<ClientStateSnapshot>;
    let (mut current, mut queued) = (None::<CurrentPlan>, None::<PendingPlan>);
    let mut observation = Some(start_observation(Arc::clone(&snapshots)));
    let mut settled_target = None::<Arc<ValidatedSnapshot>>;
    let mut local_change_pending = false;
    let mut awaiting_target = false;
    let mut snapshot_error = None::<SnapshotError>;
    let mut failed_task = None::<&'static str>;
    let mut maintenance = heartbeat_interval();
    let mut server_deadline = Instant::now() + SERVER_SILENCE_TIMEOUT;
    loop {
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
            message = socket.next() => {
                match decode_active(message, session_id) {
                    ActiveInput::Target(target) => {
                        server_deadline = Instant::now() + SERVER_SILENCE_TIMEOUT;
                        if let Err(error) = queue_latest_plan(
                            &snapshots,
                            &mut current,
                            &mut queued,
                            settled_target.as_deref(),
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
                    ActiveInput::Alive => {
                        server_deadline = Instant::now() + SERVER_SILENCE_TIMEOUT;
                    }
                    ActiveInput::Retry => {
                        break;
                    }
                }
            }
            result = plan_finished => {
                let Some(finished) = current.take() else {
                    break;
                };
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
                        failed_task = Some("reconciliation");
                        break;
                    }
                };
                settled_target = Some(finished.target);
                if local_change_pending {
                    local_change_pending = false;
                    observation = Some(start_observation(Arc::clone(&snapshots)));
                    continue;
                }
                let Ok(sent) = send_changed_snapshot(&mut socket, session_id, &mut last_sent, snapshot).await else {
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
                    observation = Some(start_observation(Arc::clone(&snapshots)));
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
                if current.is_some() || awaiting_target {
                    local_change_pending = true;
                } else if observation.is_some() {
                    local_change_pending |= !heartbeat_due;
                } else {
                    observation = Some(start_observation(Arc::clone(&snapshots)));
                }
            }
        }
    }
    drop(socket);
    if let Some(task) = observation {
        task.abort();
        let _ = task.await;
    }
    fence_plans(&snapshots, current, queued).await;
    if let Some(error) = snapshot_error {
        tracing::error!(error = %error, "Device active snapshot processing failed");
    } else if let Some(task) = failed_task {
        tracing::error!(task, "Device active snapshot task terminated unexpectedly");
    }
}

fn queue_latest_plan(
    snapshots: &Arc<SnapshotReconciler>,
    current: &mut Option<CurrentPlan>,
    pending: &mut Option<PendingPlan>,
    settled: Option<&ValidatedSnapshot>,
    awaiting_target: bool,
    observation_running: bool,
    target: ServerStateSnapshot,
) -> Result<(), SnapshotError> {
    let target = Arc::new(validate_server_snapshot(target)?);
    if target_is_redundant(
        current.as_ref().map(|plan| plan.target.as_ref()),
        pending.as_ref().map(|plan| plan.target.as_ref()),
        settled,
        awaiting_target,
        target.as_ref(),
    ) {
        return Ok(());
    }
    let fence = tokio_util::sync::CancellationToken::new();
    snapshots.begin_plan(&fence)?;
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
    settled: Option<&ValidatedSnapshot>,
    awaiting_target: bool,
    incoming: &ValidatedSnapshot,
) -> bool {
    queued.or(current).is_some_and(|target| target == incoming)
        || (queued.is_none() && current.is_none() && !awaiting_target && settled == Some(incoming))
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
) -> JoinHandle<Result<ClientStateSnapshot, SnapshotError>> {
    tokio::spawn(async move { snapshots.observe().await })
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

async fn reconnect_delay() {
    tokio::time::sleep(RECONNECT_DELAY).await;
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
    use crate::parse_endpoint;
    use natsume_device_protocol::generated::{
        BindingAccessTarget, BindingNegotiationIntent, ConcreteTargetState,
        GatewayCredentialIntent, GatewayTarget, HomeTarget, LockState, RuntimeConfigTarget,
        ServerIntentState, SessionControlTarget,
    };
    use uuid::Uuid;

    use super::*;

    #[test]
    fn control_url_uses_only_the_fixed_route_and_configured_ip_endpoint() {
        let ipv4 = parse_endpoint("192.0.2.10", "8443")
            .unwrap_or_else(|error| panic!("IPv4 fixture must parse: {error}"));
        assert_eq!(
            control_url(ipv4),
            "wss://192.0.2.10:8443/api/v2/device/control"
        );

        let ipv6 = parse_endpoint("2001:db8::1", "443")
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
        let current = target(LockState::Unlocked);
        let queued = target(LockState::Locked);

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
    fn changed_actual_allows_the_same_settled_target_to_run_again() {
        let settled = target(LockState::Unlocked);

        assert!(!target_is_redundant(
            None,
            None,
            Some(&settled),
            true,
            &settled,
        ));
    }

    fn target(lock_state: LockState) -> ValidatedSnapshot {
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
                    lock_state: lock_state.into(),
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
