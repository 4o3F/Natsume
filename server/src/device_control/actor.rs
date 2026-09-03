use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use natsume_device_protocol::generated::{
    ClientStateSnapshot, ServerActiveEnvelope, server_active_envelope,
};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::{component::device::DeviceId, server_state::ServerState};

use super::{convergence::ObservedActualState, state};

const MAILBOX_CAPACITY: usize = 8;

/// Process-local directory of the single event consumer assigned to each Device.
///
/// Entries live for the process lifetime. Reattaching a Device reuses its actor and
/// replaces the actor's current connection lease instead of spawning competing
/// consumers for the same Device.
pub(super) struct DeviceRegistry {
    devices: Mutex<HashMap<DeviceId, DeviceHandle>>,
}

impl DeviceRegistry {
    pub(super) fn new() -> Self {
        Self {
            devices: Mutex::new(HashMap::new()),
        }
    }

    /// Installs a new current lease for `device_id` and waits until its actor has
    /// observed the replacement.
    ///
    /// Once this returns, events carrying an older session ID are fenced by the
    /// actor. `None` means the actor mailbox or acknowledgement path has closed, so
    /// the caller must close the WebSocket rather than enter the active phase.
    pub(super) async fn attach(
        &self,
        device_id: DeviceId,
        outbound: mpsc::Sender<ServerActiveEnvelope>,
    ) -> Option<([u8; 16], DeviceHandle)> {
        let handle = self.get_or_spawn(device_id).await;
        let session_id = Uuid::now_v7().into_bytes();
        let (attached, received) = oneshot::channel();
        handle
            .sender
            .send(DeviceEvent::Attach {
                session_id,
                outbound,
                attached,
            })
            .await
            .ok()?;
        received.await.ok()?;
        Some((session_id, handle))
    }

    pub(super) async fn get_or_spawn(&self, device_id: DeviceId) -> DeviceHandle {
        self.devices
            .lock()
            .await
            .entry(device_id)
            .or_insert_with(|| DeviceHandle::spawn(device_id))
            .clone()
    }

    pub(super) async fn dirty_one(&self, state: Arc<ServerState>, device_id: DeviceId) {
        let handle = self.devices.lock().await.get(&device_id).cloned();
        if let Some(handle) = handle {
            handle.dirty(state);
        }
    }

    pub(super) async fn dirty_all(&self, state: Arc<ServerState>) {
        let handles = self
            .devices
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for handle in handles {
            handle.dirty(Arc::clone(&state));
        }
    }

    pub(super) async fn connection_state(&self, device_id: DeviceId) -> DeviceConnectionState {
        let handle = self.devices.lock().await.get(&device_id).cloned();
        match handle {
            Some(handle) => handle.connection_state().await,
            None => DeviceConnectionState::Offline,
        }
    }

    /// Reads current states concurrently without creating actors for unseen Devices.
    pub(super) async fn connection_states(
        &self,
        device_ids: &[DeviceId],
    ) -> HashMap<DeviceId, DeviceConnectionState> {
        let handles = {
            let devices = self.devices.lock().await;
            device_ids
                .iter()
                .filter_map(|device_id| {
                    devices
                        .get(device_id)
                        .cloned()
                        .map(|handle| (*device_id, handle))
                })
                .collect::<Vec<_>>()
        };
        let mut states = device_ids
            .iter()
            .map(|device_id| (*device_id, DeviceConnectionState::Offline))
            .collect::<HashMap<_, _>>();
        let mut queries = JoinSet::new();
        for (device_id, handle) in handles {
            queries.spawn(async move { (device_id, handle.connection_state().await) });
        }
        for (device_id, state) in queries.join_all().await {
            states.insert(device_id, state);
        }
        states
    }
}

/// Current process-local connection state exposed to the Operator query path.
///
/// `Active` contains only the latest complete, valid Actual from the current lease.
/// It is cleared on attach, disconnect, or eviction and is never persisted.
#[derive(Clone, PartialEq)]
pub(super) enum DeviceConnectionState {
    Offline,
    AwaitingFreshState,
    Active {
        actual: Box<ObservedActualState>,
        received_at_unix_ms: i64,
    },
}

/// Cloneable mailbox handle for one Device actor.
///
/// The handle does not identify a connection by itself. Every connection-scoped
/// event also carries its session ID so that a stale handle cannot mutate or close
/// the current lease.
#[derive(Clone)]
pub(super) struct DeviceHandle {
    sender: mpsc::Sender<DeviceEvent>,
    /// Excludes authority commits from in-flight component writes and fences queued work.
    pub(super) authority_fence: Arc<Mutex<bool>>,
}

impl DeviceHandle {
    /// Starts the sole event loop for a Device with a bounded mailbox.
    fn spawn(device_id: DeviceId) -> Self {
        let (sender, receiver) = mpsc::channel(MAILBOX_CAPACITY);
        let authority_fence = Arc::new(Mutex::new(false));
        tokio::spawn(run_actor(device_id, Arc::clone(&authority_fence), receiver));
        Self {
            sender,
            authority_fence,
        }
    }

    /// Queues a complete Client snapshot for the named lease.
    ///
    /// Waiting for mailbox capacity applies backpressure to the WebSocket reader.
    /// `false` means the Device actor is gone and the connection must close.
    pub(super) async fn client_state(
        &self,
        state: Arc<ServerState>,
        session_id: [u8; 16],
        snapshot: ClientStateSnapshot,
    ) -> bool {
        let Some(received_at_unix_ms) = current_unix_ms() else {
            return false;
        };
        self.sender
            .send(DeviceEvent::ClientState {
                state,
                session_id,
                snapshot: Box::new(snapshot),
                received_at_unix_ms,
            })
            .await
            .is_ok()
    }

    /// Announces connection loss without allowing an old session to clear a newer
    /// lease. Failure to enqueue is terminal only for the already-closing caller.
    pub(super) async fn disconnected(&self, session_id: [u8; 16]) {
        let _ = self
            .sender
            .send(DeviceEvent::Disconnected { session_id })
            .await;
    }

    fn dirty(&self, state: Arc<ServerState>) {
        let _ = self.sender.try_send(DeviceEvent::Dirty { state });
    }

    pub(super) async fn evict(&self) {
        let (evicted, received) = oneshot::channel();
        if self
            .sender
            .send(DeviceEvent::Evict { evicted })
            .await
            .is_ok()
        {
            let _ = received.await;
        }
    }

    async fn connection_state(&self) -> DeviceConnectionState {
        let (respond, received) = oneshot::channel();
        if self
            .sender
            .send(DeviceEvent::ConnectionState { respond })
            .await
            .is_err()
        {
            return DeviceConnectionState::Offline;
        }
        received.await.unwrap_or(DeviceConnectionState::Offline)
    }
}

/// The complete set of events serialized by one Device actor in WP8.
///
/// Keeping attach, state reconciliation, and disconnect in the same mailbox makes
/// lease replacement atomic with respect to Client snapshots. The snapshot is boxed
/// only to keep mailbox events small; it adds no separate lifecycle.
enum DeviceEvent {
    /// Replaces any current lease and acknowledges when fencing is effective.
    Attach {
        session_id: [u8; 16],
        outbound: mpsc::Sender<ServerActiveEnvelope>,
        attached: oneshot::Sender<()>,
    },
    /// Reconciles one complete snapshot if `session_id` is still current.
    ClientState {
        state: Arc<ServerState>,
        session_id: [u8; 16],
        snapshot: Box<ClientStateSnapshot>,
        received_at_unix_ms: i64,
    },
    /// Re-materializes the complete target after a committed Operator mutation.
    Dirty { state: Arc<ServerState> },
    /// Clears the current lease and acknowledges once its outbound path is closed.
    Evict { evicted: oneshot::Sender<()> },
    /// Reads the current lease observation without introducing a durable projection.
    ConnectionState {
        respond: oneshot::Sender<DeviceConnectionState>,
    },
    /// Clears the lease only when the disconnect belongs to the current session.
    Disconnected {
        /// Identifies the lease being closed, not necessarily the current lease.
        session_id: [u8; 16],
    },
}

/// Connection-scoped capability to publish Server state for a Device.
///
/// Dropping this value closes its outbound channel. Replacement, reconciliation
/// failure, outbound backpressure, and matching disconnect all terminate the lease
/// by clearing this value.
struct CurrentLease {
    session_id: [u8; 16],
    outbound: mpsc::Sender<ServerActiveEnvelope>,
    observation: Option<(ObservedActualState, i64)>,
}

/// Serial Device event loop.
///
/// A stale Client snapshot is ignored before any component call. A current snapshot
/// must reconcile into one complete Server snapshot; validation or component failure
/// drops the lease. Publishing is deliberately non-blocking: a full or closed
/// outbound queue also drops the lease rather than accumulating stale targets.
#[expect(
    clippy::too_many_lines,
    reason = "the event loop keeps the complete lease transition order visible"
)]
async fn run_actor(
    device_id: DeviceId,
    authority_fence: Arc<Mutex<bool>>,
    mut receiver: mpsc::Receiver<DeviceEvent>,
) {
    let mut current: Option<CurrentLease> = None;
    while let Some(event) = receiver.recv().await {
        match event {
            DeviceEvent::Attach {
                session_id,
                outbound,
                attached,
            } => {
                *authority_fence.lock().await = false;
                current = Some(CurrentLease {
                    session_id,
                    outbound,
                    observation: None,
                });
                let _ = attached.send(());
            }
            DeviceEvent::ClientState {
                state,
                session_id,
                snapshot,
                received_at_unix_ms,
            } => {
                let Some(outbound) = current
                    .as_ref()
                    .filter(|lease| lease.session_id == session_id)
                    .map(|lease| lease.outbound.clone())
                else {
                    continue;
                };
                let fenced = authority_fence.lock().await;
                if *fenced {
                    current = None;
                    continue;
                }
                let Some((snapshot, actual)) = state::reconcile(&state, device_id, *snapshot).await
                else {
                    current = None;
                    continue;
                };
                let envelope = ServerActiveEnvelope {
                    session_id: session_id.to_vec(),
                    body: Some(server_active_envelope::Body::ServerState(snapshot)),
                };
                if outbound.try_send(envelope).is_err() {
                    current = None;
                } else if let Some(lease) = current.as_mut() {
                    lease.observation = Some((actual, received_at_unix_ms));
                }
            }
            DeviceEvent::Dirty { state } => {
                let Some((session_id, outbound)) = current
                    .as_ref()
                    .filter(|lease| lease.observation.is_some())
                    .map(|lease| (lease.session_id, lease.outbound.clone()))
                else {
                    continue;
                };
                let fenced = authority_fence.lock().await;
                if *fenced {
                    current = None;
                    continue;
                }
                let Some(snapshot) = state::materialize(&state, device_id).await else {
                    current = None;
                    continue;
                };
                let envelope = ServerActiveEnvelope {
                    session_id: session_id.to_vec(),
                    body: Some(server_active_envelope::Body::ServerState(snapshot)),
                };
                if outbound.try_send(envelope).is_err() {
                    current = None;
                }
            }
            DeviceEvent::Evict { evicted } => {
                current = None;
                let _ = evicted.send(());
            }
            DeviceEvent::ConnectionState { respond } => {
                let state = match current.as_ref() {
                    None => DeviceConnectionState::Offline,
                    Some(lease) => match lease.observation.as_ref() {
                        None => DeviceConnectionState::AwaitingFreshState,
                        Some((actual, received_at_unix_ms)) => DeviceConnectionState::Active {
                            actual: Box::new(actual.clone()),
                            received_at_unix_ms: *received_at_unix_ms,
                        },
                    },
                };
                let _ = respond.send(state);
            }
            DeviceEvent::Disconnected { session_id } => {
                if current
                    .as_ref()
                    .is_some_and(|lease| lease.session_id == session_id)
                {
                    current = None;
                }
            }
        }
    }
}

fn current_unix_ms() -> Option<i64> {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(milliseconds).ok()
}
