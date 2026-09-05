use std::{
    collections::HashMap,
    sync::{Arc, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use natsume_device_protocol::generated::{
    ClientStateSnapshot, ServerActiveEnvelope, server_active_envelope,
};
use tokio::task::JoinSet;
use tokio::{
    sync::{Mutex, mpsc, oneshot, watch},
    time::{Instant, MissedTickBehavior},
};
use uuid::Uuid;

use crate::{component::device::DeviceId, server_state::ServerState};

use super::{convergence::ObservedActualState, state};

const MAILBOX_CAPACITY: usize = 8;
pub(super) const TARGET_REFRESH_INTERVAL: Duration = Duration::from_mins(1);

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

    pub(super) async fn get_or_spawn(&self, device_id: DeviceId) -> DeviceHandle {
        self.devices
            .lock()
            .await
            .entry(device_id)
            .or_insert_with(|| DeviceHandle::spawn_actor(device_id))
            .clone()
    }

    /// Returns an existing actor without allocating process-lifetime state.
    pub(super) async fn get(&self, device_id: DeviceId) -> Option<DeviceHandle> {
        self.devices.lock().await.get(&device_id).cloned()
    }

    pub(super) async fn dirty_one(&self, device_id: DeviceId) {
        let handle = self.devices.lock().await.get(&device_id).cloned();
        if let Some(handle) = handle {
            handle.dirty();
        }
    }

    pub(super) async fn dirty_all(&self) {
        let handles = self
            .devices
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for handle in handles {
            handle.dirty();
        }
    }

    pub(super) async fn read_connection_state(&self, device_id: DeviceId) -> DeviceConnectionState {
        let handle = self.devices.lock().await.get(&device_id).cloned();
        match handle {
            Some(handle) => handle.read_connection_state().await,
            None => DeviceConnectionState::Offline,
        }
    }

    /// Reads current states concurrently without creating actors for unseen Devices.
    pub(super) async fn read_connection_states(
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
            queries.spawn(async move { (device_id, handle.read_connection_state().await) });
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
    dirty: watch::Sender<()>,
    /// Excludes authority commits from in-flight component writes and fences queued work.
    pub(super) authority_fence: Arc<Mutex<bool>>,
}

impl DeviceHandle {
    /// Starts the sole event loop for a Device with a bounded mailbox.
    fn spawn_actor(device_id: DeviceId) -> Self {
        let (sender, receiver) = mpsc::channel(MAILBOX_CAPACITY);
        let (dirty, dirty_receiver) = watch::channel(());
        let authority_fence = Arc::new(Mutex::new(false));
        tokio::spawn(run_actor(
            device_id,
            Arc::clone(&authority_fence),
            receiver,
            dirty_receiver,
        ));
        Self {
            sender,
            dirty,
            authority_fence,
        }
    }

    /// Replaces the current connection lease and waits until stale sessions are fenced.
    pub(super) async fn replace_current_lease(
        &self,
        state: Weak<ServerState>,
        outbound: mpsc::Sender<ServerActiveEnvelope>,
    ) -> Option<Uuid> {
        let (replaced, received) = oneshot::channel();
        self.sender
            .send(DeviceEvent::ReplaceCurrentLease {
                state,
                outbound,
                replaced,
            })
            .await
            .ok()?;
        received.await.ok()
    }

    /// Queues a complete Client snapshot for the named lease.
    ///
    /// Waiting for mailbox capacity applies backpressure to the WebSocket reader. Success means
    /// only that the snapshot was queued; the actor still owns validation and reconciliation.
    pub(super) async fn enqueue_client_state(
        &self,
        session_id: Uuid,
        snapshot: ClientStateSnapshot,
    ) -> Result<(), mpsc::error::SendError<()>> {
        let received_at_unix_ms = current_unix_ms().ok_or(mpsc::error::SendError(()))?;
        self.sender
            .send(DeviceEvent::ReconcileClientState {
                session_id,
                snapshot: Box::new(snapshot),
                received_at_unix_ms,
            })
            .await
            .map_err(|_| mpsc::error::SendError(()))
    }

    /// Announces connection loss without allowing an old session to clear a newer
    /// lease. Failure to enqueue is terminal only for the already-closing caller.
    pub(super) async fn clear_lease_if_current(&self, session_id: Uuid) {
        let _ = self
            .sender
            .send(DeviceEvent::ClearLeaseIfCurrent { session_id })
            .await;
    }

    fn dirty(&self) {
        self.dirty.send_replace(());
    }

    pub(super) async fn evict_current_lease(&self) {
        let (evicted, received) = oneshot::channel();
        if self
            .sender
            .send(DeviceEvent::EvictCurrentLease { evicted })
            .await
            .is_ok()
        {
            let _ = received.await;
        }
    }

    async fn read_connection_state(&self) -> DeviceConnectionState {
        let (respond, received) = oneshot::channel();
        if self
            .sender
            .send(DeviceEvent::ReadConnectionState { respond })
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
    ReplaceCurrentLease {
        state: Weak<ServerState>,
        outbound: mpsc::Sender<ServerActiveEnvelope>,
        replaced: oneshot::Sender<Uuid>,
    },
    /// Reconciles one complete snapshot if `session_id` is still current.
    ReconcileClientState {
        session_id: Uuid,
        snapshot: Box<ClientStateSnapshot>,
        received_at_unix_ms: i64,
    },
    /// Clears the current lease and acknowledges once its outbound path is closed.
    EvictCurrentLease { evicted: oneshot::Sender<()> },
    /// Reads the current lease observation without introducing a durable projection.
    ReadConnectionState {
        respond: oneshot::Sender<DeviceConnectionState>,
    },
    /// Clears the lease only when the disconnect belongs to the current session.
    ClearLeaseIfCurrent {
        /// Identifies the lease being closed, not necessarily the current lease.
        session_id: Uuid,
    },
}

/// Connection-scoped capability to publish Server state for a Device.
///
/// Dropping this value closes its outbound channel. Replacement, reconciliation
/// failure, outbound backpressure, and matching disconnect all terminate the lease
/// by clearing this value.
struct CurrentLease {
    session_id: Uuid,
    outbound: mpsc::Sender<ServerActiveEnvelope>,
    observation: Option<(ObservedActualState, i64)>,
    state: Weak<ServerState>,
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
    mut dirty: watch::Receiver<()>,
) {
    let mut current: Option<CurrentLease> = None;
    let mut refresh = tokio::time::interval_at(
        Instant::now() + TARGET_REFRESH_INTERVAL,
        TARGET_REFRESH_INTERVAL,
    );
    refresh.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let event = tokio::select! {
            event = receiver.recv() => event,
            changed = dirty.changed() => {
                if changed.is_err() {
                    break;
                }
                dirty.borrow_and_update();
                refresh_target(device_id, &authority_fence, &mut current).await;
                continue;
            }
            _ = refresh.tick() => {
                refresh_target(device_id, &authority_fence, &mut current).await;
                continue;
            }
        };
        let Some(event) = event else {
            break;
        };
        match event {
            DeviceEvent::ReplaceCurrentLease {
                state,
                outbound,
                replaced,
            } => {
                let session_id = Uuid::now_v7();
                refresh.reset_at(Instant::now() + target_refresh_delay(&session_id));
                *authority_fence.lock().await = false;
                current = Some(CurrentLease {
                    session_id,
                    outbound,
                    observation: None,
                    state,
                });
                let _ = replaced.send(session_id);
            }
            DeviceEvent::ReconcileClientState {
                session_id,
                snapshot,
                received_at_unix_ms,
            } => {
                let Some((outbound, state)) = current
                    .as_ref()
                    .filter(|lease| lease.session_id == session_id)
                    .and_then(|lease| {
                        lease
                            .state
                            .upgrade()
                            .map(|state| (lease.outbound.clone(), state))
                    })
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
                    session_id: session_id.as_bytes().to_vec(),
                    body: Some(server_active_envelope::Body::ServerState(snapshot)),
                };
                if outbound.try_send(envelope).is_err() {
                    current = None;
                } else if let Some(lease) = current.as_mut() {
                    lease.observation = Some((actual, received_at_unix_ms));
                }
            }
            DeviceEvent::EvictCurrentLease { evicted } => {
                current = None;
                let _ = evicted.send(());
            }
            DeviceEvent::ReadConnectionState { respond } => {
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
            DeviceEvent::ClearLeaseIfCurrent { session_id } => {
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

fn target_refresh_delay(session_id: &Uuid) -> Duration {
    Duration::from_secs(u64::from(session_id.as_bytes()[15] % 60) + 1)
}

async fn refresh_target(
    device_id: DeviceId,
    authority_fence: &Mutex<bool>,
    current: &mut Option<CurrentLease>,
) {
    let Some((session_id, outbound, state)) = current
        .as_ref()
        .filter(|lease| lease.observation.is_some())
        .and_then(|lease| {
            lease
                .state
                .upgrade()
                .map(|state| (lease.session_id, lease.outbound.clone(), state))
        })
    else {
        return;
    };
    let fenced = authority_fence.lock().await;
    if *fenced {
        *current = None;
        return;
    }
    let Some(snapshot) = state::materialize(&state, device_id).await else {
        *current = None;
        return;
    };
    let envelope = ServerActiveEnvelope {
        session_id: session_id.as_bytes().to_vec(),
        body: Some(server_active_envelope::Body::ServerState(snapshot)),
    };
    if outbound.try_send(envelope).is_err() {
        *current = None;
    }
}

fn current_unix_ms() -> Option<i64> {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(milliseconds).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dirty_notification_is_independent_of_mailbox_capacity() {
        let (sender, _receiver) = mpsc::channel(MAILBOX_CAPACITY);
        for _ in 0..MAILBOX_CAPACITY {
            assert!(
                sender
                    .try_send(DeviceEvent::ClearLeaseIfCurrent {
                        session_id: Uuid::nil(),
                    })
                    .is_ok()
            );
        }
        let (dirty, mut dirty_receiver) = watch::channel(());
        let handle = DeviceHandle {
            sender,
            dirty,
            authority_fence: Arc::new(Mutex::new(false)),
        };

        handle.dirty();

        assert!(dirty_receiver.changed().await.is_ok());
    }

    #[test]
    fn target_refresh_phase_is_spread_within_one_interval() {
        let session_id = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000);
        assert_eq!(target_refresh_delay(&session_id), Duration::from_secs(1));
        let session_id = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_003b);
        assert_eq!(target_refresh_delay(&session_id), TARGET_REFRESH_INTERVAL);
    }
}
