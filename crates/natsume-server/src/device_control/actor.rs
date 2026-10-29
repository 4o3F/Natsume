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

use crate::{
    component::device::{ControlAuthority, DeviceId},
    journal::{
        Journal, JournalContext,
        event::{
            Event,
            payload::{ActorStopped, Disconnected, LeaseAttached, LeaseReplaced, ReconcileRetry},
            types::{DisconnectReason, SessionId},
        },
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FenceKind {
    Disabled,
    Revoked,
    ControlKeyReplaced,
}

#[derive(Clone)]
pub(super) struct FenceReason {
    /// Committed authority change that invalidates the lease.
    pub(super) kind: FenceKind,
    /// Explicit initiating context attached to the committed lease invalidation.
    pub(super) context: JournalContext,
}

impl FenceKind {
    pub(super) fn reason(self) -> DisconnectReason {
        match self {
            Self::Disabled => DisconnectReason::Disabled,
            Self::Revoked => DisconnectReason::Revoked,
            Self::ControlKeyReplaced => DisconnectReason::ControlKeyReplaced,
        }
    }
}

#[derive(Clone)]
pub(super) struct LeaseEnd {
    /// Closed termination cause, without transport errors or protocol contents.
    pub(super) reason: DisconnectReason,
    /// Explicit initiating context; actor-local causes use the default context.
    pub(super) context: JournalContext,
}

impl LeaseEnd {
    pub(super) fn connection(cause: super::ConnectionEnd) -> Self {
        Self {
            reason: cause.reason(),
            context: JournalContext::default(),
        }
    }
    pub(super) fn local(reason: DisconnectReason) -> Self {
        Self {
            reason,
            context: JournalContext::default(),
        }
    }
}

#[derive(Clone, Copy)]
enum EndSource {
    Connection,
    Eviction,
}

struct PendingLeaseEnd {
    /// Exact lease whose caller has observed termination, checked again before using it.
    session_id: Uuid,
    /// Explicit eviction outranks a connection-side termination intent.
    source: EndSource,
    /// Closed cause and explicit initiator facts, never an error or protocol payload.
    end: LeaseEnd,
}

#[derive(Clone)]
struct ActorJournal {
    /// Emit-only handle explicitly supplied by the device coordinator.
    journal: Journal,
    /// Stable device identity attached to all actor and supervisor events.
    device_id: DeviceId,
    /// Actor-updated current lease; retained on abnormal exit for the supervisor.
    current_session: watch::Sender<Option<SessionId>>,
    /// Caller-observed termination retained while reconciliation awaits I/O; actor owns consumption.
    pending_end: Arc<std::sync::Mutex<Option<PendingLeaseEnd>>>,
}

impl ActorJournal {
    fn context(&self, session: Uuid) -> JournalContext {
        JournalContext::default()
            .with_device(self.device_id)
            .with_session(SessionId::from_lease(session))
    }

    fn observe_end(&self, session_id: Uuid, source: EndSource, end: &LeaseEnd) {
        let current = self.current_session.borrow();
        if current
            .as_ref()
            .is_none_or(|current| current.value() != session_id)
        {
            return;
        }
        // Only a complete named value is replaced; poisoning cannot leave a partial intent.
        let mut pending = self
            .pending_end
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.as_ref().is_none_or(|pending| {
            pending.session_id != session_id
                || matches!(
                    (pending.source, source),
                    (EndSource::Connection, EndSource::Eviction)
                )
        }) {
            *pending = Some(PendingLeaseEnd {
                session_id,
                source,
                end: end.clone(),
            });
        }
    }

    fn terminate_after_fault(&self, current: &mut Option<CurrentLease>, reason: DisconnectReason) {
        let Some(session_id) = current.as_ref().map(|lease| lease.session_id) else {
            return;
        };
        // Taking one complete value preserves validity even after lock poisoning.
        let pending = self
            .pending_end
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let end = pending
            .filter(|pending| pending.session_id == session_id)
            .map_or_else(|| LeaseEnd::local(reason), |pending| pending.end);
        self.terminate(current, None, end);
    }

    fn terminate_fenced(&self, current: &mut Option<CurrentLease>, fence: &FenceReason) {
        self.terminate(
            current,
            None,
            LeaseEnd {
                reason: fence.kind.reason(),
                context: fence.context.clone(),
            },
        );
    }

    fn terminate(
        &self,
        current: &mut Option<CurrentLease>,
        fence: Option<&FenceReason>,
        end: LeaseEnd,
    ) {
        if let Some(lease) = current.take() {
            let end = match fence {
                Some(fence) => LeaseEnd {
                    reason: fence.kind.reason(),
                    context: fence.context.clone(),
                },
                None => end,
            };
            let session_id = SessionId::from_lease(lease.session_id);
            self.journal.emit(
                Event::Disconnected(Disconnected {
                    session_id,
                    reason: end.reason,
                }),
                end.context
                    .with_device(self.device_id)
                    .with_session(session_id),
            );
            self.current_session.send_replace(None);
        }
    }
}

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
    /// Journal dependency copied into each actor and its supervisor.
    journal: Journal,
}

#[derive(Default)]
struct RegistryState {
    /// Initially false; shutdown sets true permanently before draining, under the directory lock.
    closed: bool,
    /// Current actor handles; registry inserts only while open, supervisors remove by identity.
    handles: HashMap<DeviceId, DeviceHandle>,
    /// Owns all supervisors, including retired actors, until registry shutdown joins them.
    supervisors: JoinSet<()>,
}

#[derive(Clone)]
pub(crate) struct LiveLeaseView {
    /// Weak directory view; querying never prolongs coordinator or actor lifetime.
    state: Weak<Mutex<RegistryState>>,
}

impl LiveLeaseView {
    pub(crate) async fn session(&self, device_id: DeviceId) -> Option<SessionId> {
        let state = self.state.upgrade()?;
        let state = state.lock().await;
        if state.closed {
            return None;
        }
        let handle = state.handles.get(&device_id)?;
        if handle.sender.is_closed() {
            return None;
        }
        *handle.events.current_session.borrow()
    }
}

impl DeviceRegistry {
    pub(super) fn live_view(&self) -> LiveLeaseView {
        LiveLeaseView {
            state: Arc::downgrade(&self.state),
        }
    }

    pub(super) fn new(journal: Journal) -> Self {
        Self {
            state: Arc::new(Mutex::new(RegistryState::default())),
            journal,
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
        let mut supervisors = std::mem::take(&mut self.state.lock().await.supervisors);
        while let Some(result) = supervisors.join_next().await {
            if let Err(source) = result {
                tracing::warn!(error = %crate::task::TaskFailure::from(source), "Device supervisor task failed");
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
                while let Some(result) = state.supervisors.try_join_next() {
                    if let Err(source) = result {
                        tracing::warn!(error = %crate::task::TaskFailure::from(source), "Device supervisor task failed");
                    }
                }
                let RegistryState {
                    handles,
                    supervisors,
                    ..
                } = &mut *state;
                match handles.entry(device_id) {
                    Entry::Vacant(entry) => {
                        let handle = DeviceHandle::spawn_actor(
                            device_id,
                            Arc::downgrade(&self.state),
                            self.journal.clone(),
                            supervisors,
                        );
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
            // empty fence is safe because attach re-checks durable authority before and after replacement.
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
                let handle = DeviceHandle::spawn_actor(
                    device_id,
                    Arc::downgrade(&self.state),
                    self.journal.clone(),
                    &mut state.supervisors,
                );
                state.handles.insert(device_id, handle.clone());
                return Some(handle);
            }
        }
    }

    pub(super) async fn lock_current_fence(
        &self,
        device_id: DeviceId,
    ) -> Option<(DeviceHandle, OwnedMutexGuard<Option<FenceReason>>)> {
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
    pub(super) authority_fence: Arc<Mutex<Option<FenceReason>>>,
    /// Shared actor emission facts, including the current session retained for supervision.
    events: ActorJournal,
}

impl DeviceHandle {
    /// Starts the sole event loop for a Device with a bounded mailbox.
    fn spawn_actor(
        device_id: DeviceId,
        registry: Weak<Mutex<RegistryState>>,
        journal: Journal,
        supervisors: &mut JoinSet<()>,
    ) -> Self {
        let (handle, supervisor) = Self::spawn_actor_with(
            device_id,
            registry,
            journal,
            move |fence, receiver, dirty, events| {
                run_actor(device_id, fence, receiver, dirty, events)
            },
        );
        supervisors.spawn(supervisor.with_current_subscriber());
        handle
    }

    fn spawn_actor_with<F>(
        device_id: DeviceId,
        registry: Weak<Mutex<RegistryState>>,
        journal: Journal,
        run: impl FnOnce(
            Arc<Mutex<Option<FenceReason>>>,
            mpsc::Receiver<DeviceEvent>,
            watch::Receiver<()>,
            ActorJournal,
        ) -> F,
    ) -> (Self, impl Future<Output = ()> + Send + 'static)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel(MAILBOX_CAPACITY);
        let (dirty, dirty_receiver) = watch::channel(());
        let (current_session, _) = watch::channel(None);
        let handle = Self {
            sender,
            dirty,
            stopping: Arc::new(AtomicBool::new(false)),
            authority_fence: Arc::new(Mutex::new(None)),
            events: ActorJournal {
                journal,
                device_id,
                current_session,
                pending_end: Arc::new(std::sync::Mutex::new(None)),
            },
        };
        let actor = tokio::spawn(
            run(
                Arc::clone(&handle.authority_fence),
                receiver,
                dirty_receiver,
                handle.events.clone(),
            )
            .with_current_subscriber(),
        );
        let supervisor = supervise_actor(
            handle.events.clone(),
            registry,
            handle.sender.downgrade(),
            Arc::clone(&handle.authority_fence),
            Arc::clone(&handle.stopping),
            actor,
        );
        (handle, supervisor)
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
    pub(super) async fn clear_lease_if_current(
        &self,
        session_id: Uuid,
        cause: super::ConnectionEnd,
    ) {
        self.end_lease_if_current(session_id, LeaseEnd::connection(cause))
            .await;
    }

    pub(super) async fn end_lease_if_current(&self, session_id: Uuid, cause: LeaseEnd) {
        self.events
            .observe_end(session_id, EndSource::Connection, &cause);
        let _ = self
            .sender
            .send(DeviceEvent::ClearLeaseIfCurrent { session_id, cause })
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

    pub(super) async fn evict_current_lease(&self, end: LeaseEnd) {
        let session = *self.events.current_session.borrow();
        if let Some(session) = session {
            self.events
                .observe_end(session.value(), EndSource::Eviction, &end);
        }
        let (evicted, received) = oneshot::channel();
        if self
            .sender
            .send(DeviceEvent::EvictCurrentLease { evicted, end })
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
    events: ActorJournal,
    registry: Weak<Mutex<RegistryState>>,
    sender: mpsc::WeakSender<DeviceEvent>,
    authority_fence: Arc<Mutex<Option<FenceReason>>>,
    stopping: Arc<AtomicBool>,
    actor: JoinHandle<()>,
) {
    let result = actor.await;
    if result.is_ok() && (sender.strong_count() == 0 || stopping.load(Ordering::Acquire)) {
        return;
    }
    let device_id = events.device_id;
    let session_id = *events.current_session.borrow();
    let mut context = JournalContext::default().with_device(device_id);
    if let Some(session_id) = session_id {
        context = context.with_session(session_id);
    }
    events.journal.emit(
        Event::ActorStopped(ActorStopped {
            panicked: result.as_ref().is_err_and(tokio::task::JoinError::is_panic),
            session_id,
        }),
        context,
    );
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
        /// Explicit committed eviction reason and initiating context.
        end: LeaseEnd,
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
        /// Explicit termination facts; a committed fence takes precedence.
        cause: LeaseEnd,
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
    authority_fence: Arc<Mutex<Option<FenceReason>>>,
    mut receiver: mpsc::Receiver<DeviceEvent>,
    mut dirty: watch::Receiver<()>,
    events: ActorJournal,
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
                let fenced = authority_fence.lock().await;
                if let Some(reason) = fenced.as_ref() {
                    events.terminate_fenced(&mut current, reason);
                } else if current.as_ref().is_some_and(|lease| lease.consecutive_failures == 0) {
                    drop(fenced);
                    refresh_target(device_id, &authority_fence, &mut current, &mut refresh, &events).await;
                }
                continue;
            }
            _ = refresh.tick() => {
                refresh_target(device_id, &authority_fence, &mut current, &mut refresh, &events).await;
                continue;
            }
        };
        let Some(event) = event else {
            break;
        };
        match event {
            DeviceEvent::Shutdown { stopped } => {
                let fence = authority_fence.lock().await;
                terminate_for_server_shutdown(&events, &mut current, fence.as_ref());
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
                if fenced.is_some() {
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
                if let Some(reason) = fence.as_ref() {
                    events.terminate_fenced(&mut current, reason);
                }
                let previous = current.take();
                let session_id = Uuid::now_v7();
                let new = SessionId::from_lease(session_id);
                match previous {
                    Some(lease) => events.journal.emit(
                        Event::LeaseReplaced(LeaseReplaced {
                            old: SessionId::from_lease(lease.session_id),
                            new,
                        }),
                        events.context(session_id),
                    ),
                    None => events.journal.emit(
                        Event::LeaseAttached(LeaseAttached { session_id: new }),
                        events.context(session_id),
                    ),
                }
                *fence = None;
                drop(fence);
                events.current_session.send_replace(Some(new));
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
                    refresh_target(
                        device_id,
                        &authority_fence,
                        &mut current,
                        &mut refresh,
                        &events,
                    )
                    .await;
                }
            }
            DeviceEvent::EvictCurrentLease { evicted, end } => {
                let fence = authority_fence.lock().await;
                events.terminate(&mut current, fence.as_ref(), end);
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
            DeviceEvent::ClearLeaseIfCurrent { session_id, cause } => {
                if current
                    .as_ref()
                    .is_some_and(|lease| lease.session_id == session_id)
                {
                    let fence = authority_fence.lock().await;
                    events.terminate(&mut current, fence.as_ref(), cause);
                }
            }
        }
    }
}

fn terminate_for_server_shutdown(
    events: &ActorJournal,
    current: &mut Option<CurrentLease>,
    fence: Option<&FenceReason>,
) {
    events.terminate(
        current,
        fence,
        LeaseEnd::local(DisconnectReason::ServerShutdown),
    );
}

fn target_refresh_delay(session_id: &Uuid) -> Duration {
    Duration::from_secs(u64::from(session_id.as_bytes()[15] % 60) + 1)
}

async fn refresh_target(
    device_id: DeviceId,
    authority_fence: &Mutex<Option<FenceReason>>,
    current: &mut Option<CurrentLease>,
    refresh: &mut tokio::time::Interval,
    events: &ActorJournal,
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
    if let Some(reason) = fenced.as_ref() {
        events.terminate_fenced(current, reason);
        return;
    }
    let pending = current.as_mut().and_then(|lease| lease.pending.take());
    let result = match pending.as_ref() {
        Some((snapshot, received_at)) => {
            state::reconcile(&control, device_id, snapshot, events.context(session_id))
                .await
                .map(|(snapshot, actual)| (snapshot, Some((actual, *received_at))))
        }
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
            reconcile_failed(device_id, error, current, refresh, events);
            return;
        }
    };
    let envelope = ServerActiveEnvelope {
        session_id: session_id.as_bytes().to_vec(),
        body: Some(server_active_envelope::Body::ServerState(snapshot)),
    };
    if let Err(error) = outbound.try_send(envelope) {
        let reason = match error {
            mpsc::error::TrySendError::Full(_) => DisconnectReason::OutboundBackpressure,
            mpsc::error::TrySendError::Closed(_) => DisconnectReason::OutboundClosed,
        };
        events.terminate_after_fault(current, reason);
    } else if let Some(lease) = current.as_mut() {
        if let Some(observation) = observation {
            observation.0.emit_journal_changes(
                lease.observation.as_ref().map(|previous| &previous.0),
                &events.journal,
                &events.context(session_id),
            );
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
    events: &ActorJournal,
) {
    let Some(lease) = current.as_mut() else {
        return;
    };
    match error.fault() {
        state::ReconcileFault::Client => {
            tracing::info!(device_id = %device_id.as_text(), error = %snafu::Report::from_error(error), "Device Client snapshot rejected");
            events.terminate_after_fault(current, DisconnectReason::ClientFault);
        }
        state::ReconcileFault::Server => {
            tracing::error!(device_id = %device_id.as_text(), error = %snafu::Report::from_error(error), "Device reconciliation failed");
            events.terminate_after_fault(current, DisconnectReason::ServerFault);
        }
        state::ReconcileFault::ServerTransient => {
            lease.consecutive_failures += 1;
            if lease.consecutive_failures == 1
                || lease.consecutive_failures == TRANSIENT_FAILURE_LIMIT
            {
                events.journal.emit(
                    Event::ReconcileRetry(ReconcileRetry {
                        attempt: u32::from(lease.consecutive_failures),
                    }),
                    events.context(lease.session_id),
                );
            }
            if lease.consecutive_failures >= TRANSIENT_FAILURE_LIMIT {
                tracing::error!(device_id = %device_id.as_text(), consecutive_failures = lease.consecutive_failures, error = %snafu::Report::from_error(error), "Device transient failure limit reached");
                events.terminate_after_fault(current, DisconnectReason::TransientLimit);
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
pub(super) use tests::{spawn_panicking_actor, spawn_test_actor};

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
            registry.journal.clone(),
            move |_fence, receiver, dirty, _events| async move {
                let _receiver = receiver;
                let _dirty = dirty;
                future.await;
            },
        );
        state.handles.insert(device_id, handle.clone());
        Some((handle, supervisor))
    }

    pub(in crate::device_control) async fn spawn_panicking_actor(
        registry: &DeviceRegistry,
        device_id: DeviceId,
        trigger: oneshot::Receiver<()>,
    ) -> Option<(DeviceHandle, impl Future<Output = ()> + Send + 'static)> {
        let mut state = registry.state.lock().await;
        if state.closed {
            return None;
        }
        let (handle, supervisor) = DeviceHandle::spawn_actor_with(
            device_id,
            Arc::downgrade(&registry.state),
            registry.journal.clone(),
            move |fence, receiver, dirty, events| async move {
                tokio::select! {
                    () = run_actor(device_id, fence, receiver, dirty, events) => {},
                    _ = trigger => panic!("actor-session-secret-canary"),
                }
            },
        );
        state.handles.insert(device_id, handle.clone());
        Some((handle, supervisor))
    }

    #[tokio::test]
    async fn shutdown_waits_for_a_late_supervisor_even_after_actor_acknowledgement() {
        let (journal, mut receiver) = crate::journal::JournalComponent::channel(16);
        let registry = DeviceRegistry::new(journal.emitter());
        let device_id =
            DeviceId::parse(&Uuid::now_v7().to_string()).unwrap_or_else(|| panic!("device"));
        let (release, released) = oneshot::channel();
        let (entered, entering) = oneshot::channel();
        let (handle, supervisor) = DeviceHandle::spawn_actor_with(
            device_id,
            Arc::downgrade(&registry.state),
            journal.emitter(),
            move |_fence, mut receiver, _dirty, _events| async move {
                if let Some(DeviceEvent::Shutdown { stopped }) = receiver.recv().await {
                    let _ = stopped.send(());
                    let _ = entered.send(());
                    let _ = released.await;
                    panic!("supervisor-panic-canary");
                }
            },
        );
        {
            let mut state = registry.state.lock().await;
            state.handles.insert(device_id, handle);
            state.supervisors.spawn(supervisor);
        }
        let shutdown = registry.shutdown();
        tokio::pin!(shutdown);
        assert!(futures_util::poll!(&mut shutdown).is_pending());
        tokio::select! {
            () = &mut shutdown => panic!("shutdown returned before supervision"),
            _ = entering => {},
        }
        assert!(
            futures_util::poll!(&mut shutdown).is_pending(),
            "supervisor must finish before journal closes"
        );
        let _ = release.send(());
        tokio::time::timeout(Duration::from_secs(3), shutdown)
            .await
            .unwrap_or_else(|error| panic!("shutdown: {error}"));
        assert!(
            crate::journal::tests::drain_events(&mut receiver)
                .iter()
                .any(|record| matches!(record.event, Event::ActorStopped(_)))
        );
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
            let registry =
                DeviceRegistry::new(crate::journal::JournalComponent::channel(32).0.emitter());
            let device_id = DeviceId::parse("01900000-0000-7000-8000-000000000001")
                .unwrap_or_else(|| panic!("device ID"));
            let (handle, supervisor) = {
                let mut state = registry.state.lock().await;
                assert!(!state.closed);
                let (handle, supervisor) = DeviceHandle::spawn_actor_with(
                    device_id,
                    Arc::downgrade(&registry.state),
                    registry.journal.clone(),
                    move |fence, receiver, dirty, events| {
                        run_actor(device_id, fence, receiver, dirty, events)
                    },
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
                        cause: LeaseEnd::connection(super::super::ConnectionEnd::ClientClose),
                    })
                    .is_ok()
            );
        }
        let (dirty, mut dirty_receiver) = watch::channel(());
        let handle = DeviceHandle {
            sender,
            dirty,
            stopping: Arc::new(AtomicBool::new(false)),
            authority_fence: Arc::new(Mutex::new(None)),
            events: ActorJournal {
                journal: crate::journal::JournalComponent::channel(32).0.emitter(),
                device_id: DeviceId::parse("01900000-0000-7000-8000-000000000001")
                    .unwrap_or_else(|| panic!("device ID")),
                current_session: watch::channel(None).0,
                pending_end: Arc::new(std::sync::Mutex::new(None)),
            },
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
