use std::{
    collections::{HashMap, hash_map::Entry},
    future::Future,
    net::IpAddr,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use natsume_workstation_protocol::generated::{
    ClientStateSnapshot, ServerActiveEnvelope, server_active_envelope,
};
use tokio::task::{JoinHandle, JoinSet};
use tokio::{
    sync::{Mutex, OwnedMutexGuard, mpsc, oneshot, watch},
    time::{Instant, MissedTickBehavior},
};
use tracing::instrument::WithSubscriber as _;
use uuid::Uuid;

use crate::component::device::{ControlAuthority, DeviceId};

use super::{DeviceControl, convergence::ObservedActualState, state};

const MAILBOX_CAPACITY: usize = 8;
pub(super) const TARGET_REFRESH_INTERVAL: Duration = Duration::from_mins(1);
pub(super) const TRANSIENT_RETRY_DELAY: Duration = Duration::from_secs(5);
pub(super) const TRANSIENT_FAILURE_LIMIT: u8 = 3;

/// Process-local directory of the single event consumer assigned to each Device.
///
/// Healthy entries live until permanent registry closure. Reattaching a Device reuses its actor and
/// replaces the actor's current connection lease instead of spawning competing
/// consumers for the same Device.
pub(super) struct DeviceRegistry {
    /// Directory and closure latch; insertion, identity checks and draining share this mutex.
    state: Arc<Mutex<RegistryState>>,
}

#[derive(Default)]
struct RegistryState {
    /// Initially false; shutdown sets true permanently before draining, under the directory lock.
    closed: bool,
    /// Current actor handles; registry inserts only while open, supervisors remove by identity.
    handles: HashMap<DeviceId, DeviceHandle>,
}

impl DeviceRegistry {
    pub(super) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RegistryState::default())),
        }
    }

    pub(super) async fn shutdown(&self) {
        let handles = {
            let mut state = self.state.lock().await;
            state.closed = true;
            state
                .handles
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        let mut stopped = JoinSet::new();
        for handle in handles {
            stopped.spawn(async move { handle.shutdown().await });
        }
        while let Some(result) = stopped.join_next().await {
            if let Err(source) = result {
                tracing::warn!(error = %crate::task::TaskFailure::from(source), "Device shutdown task failed");
            }
        }
    }

    pub(super) async fn is_closed(&self) -> bool {
        self.state.lock().await.closed
    }

    pub(super) async fn get_or_spawn(&self, device_id: DeviceId) -> Option<DeviceHandle> {
        loop {
            let retired = {
                let mut state = self.state.lock().await;
                if state.closed {
                    return None;
                }
                match state.handles.entry(device_id) {
                    Entry::Vacant(entry) => {
                        let handle =
                            DeviceHandle::spawn_actor(device_id, Arc::downgrade(&self.state));
                        entry.insert(handle.clone());
                        return Some(handle);
                    }
                    Entry::Occupied(entry) if !entry.get().sender.is_closed() => {
                        return Some(entry.get().clone());
                    }
                    Entry::Occupied(entry) => entry.get().clone(),
                }
            };
            // Cleanup and respawn wait for the old fence's authority commit. A fresh
            // false fence is safe because attach re-checks durable authority before and after replacement.
            let _fenced = retired.authority_fence.lock().await;
            let mut state = self.state.lock().await;
            if state.closed {
                return None;
            }
            if state
                .handles
                .get(&device_id)
                .is_some_and(|current| current.sender.same_channel(&retired.sender))
            {
                tracing::warn!(device_id = %device_id.as_text(), "Replacing closed Device actor mailbox");
                let handle = DeviceHandle::spawn_actor(device_id, Arc::downgrade(&self.state));
                state.handles.insert(device_id, handle.clone());
                return Some(handle);
            }
        }
    }

    pub(super) async fn lock_current_fence(
        &self,
        device_id: DeviceId,
    ) -> Option<(DeviceHandle, OwnedMutexGuard<bool>)> {
        loop {
            let handle = match self.get(device_id).await {
                Some(handle) => handle,
                None => self.get_or_spawn(device_id).await?,
            };
            let fenced = Arc::clone(&handle.authority_fence).lock_owned().await;
            // Mutation, replacement and supervisor cleanup all lock fence before registry.
            // Rechecking identity closes the lookup gap; replacement and cleanup require
            // the same fence, while shutdown can only remove entries after permanent closure.
            let state = self.state.lock().await;
            if state.closed {
                return None;
            }
            if state
                .handles
                .get(&device_id)
                .is_some_and(|current| current.sender.same_channel(&handle.sender))
            {
                return Some((handle, fenced));
            }
        }
    }

    /// Returns an existing actor without allocating process-lifetime state.
    pub(super) async fn get(&self, device_id: DeviceId) -> Option<DeviceHandle> {
        self.state.lock().await.handles.get(&device_id).cloned()
    }

    pub(super) async fn dirty_one(&self, device_id: DeviceId) {
        let handle = self.state.lock().await.handles.get(&device_id).cloned();
        if let Some(handle) = handle {
            handle.dirty();
        }
    }

    pub(super) async fn dirty_many(&self, ids: &[DeviceId]) {
        let handles = {
            let state = self.state.lock().await;
            ids.iter()
                .filter_map(|id| state.handles.get(id).cloned())
                .collect::<Vec<_>>()
        };
        for handle in handles {
            handle.dirty();
        }
    }

    pub(super) async fn dirty_all(&self) {
        let handles = self
            .state
            .lock()
            .await
            .handles
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for handle in handles {
            handle.dirty();
        }
    }

    pub(super) async fn read_connection_state(&self, device_id: DeviceId) -> DeviceConnectionState {
        let handle = self.state.lock().await.handles.get(&device_id).cloned();
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
            let state = self.state.lock().await;
            device_ids
                .iter()
                .filter_map(|device_id| {
                    state
                        .handles
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

    pub(super) async fn active_device_ids(&self) -> Vec<DeviceId> {
        let handles = self
            .state
            .lock()
            .await
            .handles
            .iter()
            .map(|(id, handle)| (*id, handle.clone()))
            .collect::<Vec<_>>();
        let mut queries = JoinSet::new();
        for (device_id, handle) in handles {
            queries.spawn(async move { (device_id, handle.read_connection_state().await) });
        }
        let mut ids = Vec::new();
        for (device_id, state) in queries.join_all().await {
            if matches!(state, DeviceConnectionState::Active { .. }) {
                ids.push(device_id);
            }
        }
        ids
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
    /// Bounded actor mailbox; closure means the actor has stopped consuming events.
    sender: mpsc::Sender<DeviceEvent>,
    /// Coalescing target-refresh notifications sent by the coordinator, consumed by the actor.
    dirty: watch::Sender<()>,
    /// Initially false; handle shutdown sets true so the supervisor recognizes expected actor exit.
    stopping: Arc<AtomicBool>,
    /// Excludes authority commits from in-flight component writes and fences queued work.
    pub(super) authority_fence: Arc<Mutex<bool>>,
}

impl DeviceHandle {
    /// Starts the sole event loop for a Device with a bounded mailbox.
    fn spawn_actor(device_id: DeviceId, registry: Weak<Mutex<RegistryState>>) -> Self {
        let (handle, supervisor) =
            Self::spawn_actor_with(device_id, registry, move |fence, receiver, dirty| {
                run_actor(device_id, fence, receiver, dirty)
            });
        tokio::spawn(supervisor.with_current_subscriber());
        handle
    }

    fn spawn_actor_with<F>(
        device_id: DeviceId,
        registry: Weak<Mutex<RegistryState>>,
        run: impl FnOnce(Arc<Mutex<bool>>, mpsc::Receiver<DeviceEvent>, watch::Receiver<()>) -> F,
    ) -> (Self, impl Future<Output = ()> + Send + 'static)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel(MAILBOX_CAPACITY);
        let (dirty, dirty_receiver) = watch::channel(());
        let authority_fence = Arc::new(Mutex::new(false));
        let stopping = Arc::new(AtomicBool::new(false));
        let actor = tokio::spawn(
            run(Arc::clone(&authority_fence), receiver, dirty_receiver).with_current_subscriber(),
        );
        let supervisor = supervise_actor(
            device_id,
            registry,
            sender.downgrade(),
            Arc::clone(&authority_fence),
            Arc::clone(&stopping),
            actor,
        );
        (
            Self {
                sender,
                dirty,
                stopping,
                authority_fence,
            },
            supervisor,
        )
    }

    /// Replaces the current connection lease and waits until stale sessions are fenced.
    pub(super) async fn replace_current_lease(
        &self,
        control: Weak<DeviceControl>,
        outbound: mpsc::Sender<ServerActiveEnvelope>,
    ) -> Option<Uuid> {
        let (replaced, received) = oneshot::channel();
        self.sender
            .send(DeviceEvent::ReplaceCurrentLease {
                control,
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
            // SendError owns the rejected snapshot; never retain protocol contents in an error.
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

    pub(super) async fn record_network_addresses(
        &self,
        session_id: Uuid,
        authority: ControlAuthority,
        server_ip: IpAddr,
        client_ip: Option<IpAddr>,
    ) {
        let _ = self
            .sender
            .send(DeviceEvent::RecordNetworkAddresses {
                session_id,
                authority,
                server_ip,
                client_ip,
            })
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

    async fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        let (stopped, received) = oneshot::channel();
        if self
            .sender
            .send(DeviceEvent::Shutdown { stopped })
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

async fn supervise_actor(
    device_id: DeviceId,
    registry: Weak<Mutex<RegistryState>>,
    sender: mpsc::WeakSender<DeviceEvent>,
    authority_fence: Arc<Mutex<bool>>,
    stopping: Arc<AtomicBool>,
    actor: JoinHandle<()>,
) {
    let result = actor.await;
    if result.is_ok() && (sender.strong_count() == 0 || stopping.load(Ordering::Acquire)) {
        return;
    }
    // JoinError can display a secret panic payload; report only the failure classification.
    tracing::error!(device_id = %device_id.as_text(), panicked = result.as_ref().is_err_and(tokio::task::JoinError::is_panic), "Device actor stopped unexpectedly");
    drop(result);
    let _fenced = authority_fence.lock().await;
    let Some(state) = registry.upgrade() else {
        return;
    };
    let Some(sender) = sender.upgrade() else {
        return;
    };
    let mut state = state.lock().await;
    if state
        .handles
        .get(&device_id)
        .is_some_and(|current| current.sender.same_channel(&sender))
    {
        state.handles.remove(&device_id);
    }
}

/// The complete set of events serialized by one Device actor in WP8.
///
/// Keeping attach, state reconciliation, and disconnect in the same mailbox makes
/// lease replacement atomic with respect to Client snapshots. The snapshot is boxed
/// only to keep mailbox events small; it adds no separate lifecycle.
enum DeviceEvent {
    Shutdown {
        stopped: oneshot::Sender<()>,
    },
    /// Persists metadata only while its authenticated lease remains current.
    RecordNetworkAddresses {
        session_id: Uuid,
        authority: ControlAuthority,
        server_ip: IpAddr,
        client_ip: Option<IpAddr>,
    },
    /// Replaces any current lease and acknowledges when fencing is effective.
    ReplaceCurrentLease {
        control: Weak<DeviceControl>,
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
    EvictCurrentLease {
        evicted: oneshot::Sender<()>,
    },
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
/// Dropping this value closes its outbound channel. Replacement, terminal faults,
/// exhausted transient retries, outbound backpressure, and matching disconnect
/// terminate the lease and discard any pending complete snapshot.
struct CurrentLease {
    session_id: Uuid,
    outbound: mpsc::Sender<ServerActiveEnvelope>,
    observation: Option<(ObservedActualState, i64)>,
    pending: Option<(Box<ClientStateSnapshot>, i64)>,
    consecutive_failures: u8,
    control: Weak<DeviceControl>,
}

/// Serial Device event loop.
///
/// Stale snapshots are ignored before component calls. Client faults and permanent
/// Server faults close the lease. Transient persistence faults retain the newest
/// complete snapshot and retry after five seconds; three consecutive faults close
/// the lease. Success clears pending work and the shared failure counter. Authority
/// fencing and outbound backpressure still terminate the lease immediately.
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
                let fenced = *authority_fence.lock().await;
                if fenced {
                    current = None;
                } else if current.as_ref().is_some_and(|lease| lease.consecutive_failures == 0) {
                    refresh_target(device_id, &authority_fence, &mut current, &mut refresh).await;
                }
                continue;
            }
            _ = refresh.tick() => {
                refresh_target(device_id, &authority_fence, &mut current, &mut refresh).await;
                continue;
            }
        };
        let Some(event) = event else {
            break;
        };
        match event {
            DeviceEvent::Shutdown { stopped } => {
                terminate_for_server_shutdown(&mut current);
                let _ = stopped.send(());
                break;
            }
            DeviceEvent::RecordNetworkAddresses {
                session_id,
                authority,
                server_ip,
                client_ip,
            } => {
                let Some(control) = current
                    .as_ref()
                    .filter(|lease| lease.session_id == session_id)
                    .and_then(|lease| lease.control.upgrade())
                else {
                    continue;
                };
                let fenced = authority_fence.lock().await;
                if *fenced {
                    continue;
                }
                if let Err(error) = control
                    .device
                    .record_network_addresses(authority, server_ip, client_ip)
                    .await
                {
                    tracing::warn!(device_id = %device_id.as_text(), error = %snafu::Report::from_error(error), "Device connection addresses could not be saved");
                }
            }
            DeviceEvent::ReplaceCurrentLease {
                control,
                outbound,
                replaced,
            } => {
                let mut fence = authority_fence.lock().await;
                *fence = false;
                drop(fence);
                let session_id = Uuid::now_v7();
                refresh.reset_at(Instant::now() + target_refresh_delay(&session_id));
                current = Some(CurrentLease {
                    session_id,
                    outbound,
                    observation: None,
                    pending: None,
                    consecutive_failures: 0,
                    control,
                });
                let _ = replaced.send(session_id);
            }
            DeviceEvent::ReconcileClientState {
                session_id,
                snapshot,
                received_at_unix_ms,
            } => {
                if let Some(lease) = current
                    .as_mut()
                    .filter(|lease| lease.session_id == session_id)
                {
                    lease.pending = Some((snapshot, received_at_unix_ms));
                    refresh_target(device_id, &authority_fence, &mut current, &mut refresh).await;
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

fn terminate_for_server_shutdown(current: &mut Option<CurrentLease>) {
    current.take();
}

fn target_refresh_delay(session_id: &Uuid) -> Duration {
    Duration::from_secs(u64::from(session_id.as_bytes()[15] % 60) + 1)
}

async fn refresh_target(
    device_id: DeviceId,
    authority_fence: &Mutex<bool>,
    current: &mut Option<CurrentLease>,
    refresh: &mut tokio::time::Interval,
) {
    let Some((session_id, outbound, control)) = current
        .as_ref()
        .filter(|lease| lease.observation.is_some() || lease.pending.is_some())
        .and_then(|lease| {
            lease
                .control
                .upgrade()
                .map(|control| (lease.session_id, lease.outbound.clone(), control))
        })
    else {
        return;
    };
    let fenced = authority_fence.lock().await;
    if *fenced {
        *current = None;
        return;
    }
    let pending = current.as_mut().and_then(|lease| lease.pending.take());
    let result = match pending.as_ref() {
        Some((snapshot, received_at)) => state::reconcile(&control, device_id, snapshot)
            .await
            .map(|(snapshot, actual)| (snapshot, Some((actual, *received_at)))),
        None => state::materialize(&control, device_id)
            .await
            .map(|snapshot| (snapshot, None)),
    };
    let (snapshot, observation) = match result {
        Ok(value) => value,
        Err(error) => {
            if let Some(lease) = current.as_mut() {
                lease.pending = pending;
            }
            reconcile_failed(device_id, error, current, refresh);
            return;
        }
    };
    let envelope = ServerActiveEnvelope {
        session_id: session_id.as_bytes().to_vec(),
        body: Some(server_active_envelope::Body::ServerState(snapshot)),
    };
    if outbound.try_send(envelope).is_err() {
        *current = None;
    } else if let Some(lease) = current.as_mut() {
        if let Some(observation) = observation {
            lease.observation = Some(observation);
        }
        if lease.consecutive_failures != 0 {
            refresh.reset_at(Instant::now() + TARGET_REFRESH_INTERVAL);
        }
        lease.consecutive_failures = 0;
    }
}

fn reconcile_failed(
    device_id: DeviceId,
    error: state::ReconcileError,
    current: &mut Option<CurrentLease>,
    refresh: &mut tokio::time::Interval,
) {
    let Some(lease) = current.as_mut() else {
        return;
    };
    match error.fault() {
        state::ReconcileFault::Client => {
            tracing::info!(device_id = %device_id.as_text(), error = %snafu::Report::from_error(error), "Device Client snapshot rejected");
            *current = None;
        }
        state::ReconcileFault::Server => {
            tracing::error!(device_id = %device_id.as_text(), error = %snafu::Report::from_error(error), "Device reconciliation failed");
            *current = None;
        }
        state::ReconcileFault::ServerTransient => {
            lease.consecutive_failures += 1;
            if lease.consecutive_failures >= TRANSIENT_FAILURE_LIMIT {
                tracing::error!(device_id = %device_id.as_text(), consecutive_failures = lease.consecutive_failures, error = %snafu::Report::from_error(error), "Device transient failure limit reached");
                *current = None;
            } else {
                tracing::warn!(device_id = %device_id.as_text(), consecutive_failures = lease.consecutive_failures, error = %snafu::Report::from_error(error), "Device reconciliation will retry");
                refresh.reset_at(Instant::now() + TRANSIENT_RETRY_DELAY);
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

#[cfg(test)]
pub(super) use tests::spawn_test_actor;

#[cfg(test)]
mod tests {
    use super::*;

    pub(in crate::device_control) async fn spawn_test_actor(
        registry: &DeviceRegistry,
        device_id: DeviceId,
        future: impl Future<Output = ()> + Send + 'static,
    ) -> Option<(DeviceHandle, impl Future<Output = ()> + Send + 'static)> {
        let mut state = registry.state.lock().await;
        if state.closed {
            return None;
        }
        let (handle, supervisor) = DeviceHandle::spawn_actor_with(
            device_id,
            Arc::downgrade(&registry.state),
            move |_fence, receiver, dirty| async move {
                let _receiver = receiver;
                let _dirty = dirty;
                future.await;
            },
        );
        state.handles.insert(device_id, handle.clone());
        Some((handle, supervisor))
    }

    #[tokio::test]
    async fn supervision_normal_registry_shutdown_is_silent_and_does_not_keep_handles_alive() {
        use crate::{
            config::LogLevel,
            logging::tests::{CapturedLogs, SubscriberTestGuard},
        };

        let _guard = SubscriberTestGuard::acquire();
        let captured = CapturedLogs::default();
        async {
            let registry = DeviceRegistry::new();
            let device_id = DeviceId::parse("01900000-0000-7000-8000-000000000001")
                .unwrap_or_else(|| panic!("device ID"));
            let (handle, supervisor) = {
                let mut state = registry.state.lock().await;
                assert!(!state.closed);
                let (handle, supervisor) = DeviceHandle::spawn_actor_with(
                    device_id,
                    Arc::downgrade(&registry.state),
                    move |fence, receiver, dirty| run_actor(device_id, fence, receiver, dirty),
                );
                state.handles.insert(device_id, handle.clone());
                (handle, supervisor)
            };
            let sender = handle.sender.downgrade();
            drop(registry);
            drop(handle);
            tokio::time::timeout(Duration::from_secs(5), supervisor)
                .await
                .unwrap_or_else(|_| panic!("normal shutdown timeout"));
            assert_eq!(sender.strong_count(), 0);
        }
        .with_subscriber(captured.subscriber(LogLevel::Trace))
        .await;
        let events = captured.text().unwrap_or_else(|()| panic!("logs"));
        assert!(!events.contains("Device actor stopped unexpectedly"));
        assert!(!events.contains("Replacing closed Device actor mailbox"));
    }

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
            stopping: Arc::new(AtomicBool::new(false)),
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
