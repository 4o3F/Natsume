use std::{collections::HashMap, sync::Arc};

use natsume_device_protocol::generated::{
    ClientStateSnapshot, ServerActiveEnvelope, server_active_envelope,
};
use tokio::sync::{Mutex, mpsc, oneshot};
use uuid::Uuid;

use crate::{component::device::DeviceId, server_state::ServerState};

use super::state;

const MAILBOX_CAPACITY: usize = 8;

/// Process-local directory of the single event consumer assigned to each Device.
///
/// Entries live for the process lifetime. Reattaching a Device reuses its actor and
/// replaces the actor's current connection lease instead of spawning competing
/// consumers for the same Device.
pub(crate) struct DeviceRegistry {
    devices: Mutex<HashMap<DeviceId, DeviceHandle>>,
}

impl DeviceRegistry {
    pub(crate) fn new() -> Self {
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
        let handle = {
            let mut devices = self.devices.lock().await;
            devices
                .entry(device_id)
                .or_insert_with(|| DeviceHandle::spawn(device_id))
                .clone()
        };
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
}

/// Cloneable mailbox handle for one Device actor.
///
/// The handle does not identify a connection by itself. Every connection-scoped
/// event also carries its session ID so that a stale handle cannot mutate or close
/// the current lease.
#[derive(Clone)]
pub(super) struct DeviceHandle {
    sender: mpsc::Sender<DeviceEvent>,
}

impl DeviceHandle {
    /// Starts the sole event loop for a Device with a bounded mailbox.
    fn spawn(device_id: DeviceId) -> Self {
        let (sender, receiver) = mpsc::channel(MAILBOX_CAPACITY);
        tokio::spawn(run_actor(device_id, receiver));
        Self { sender }
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
        self.sender
            .send(DeviceEvent::ClientState {
                state,
                session_id,
                snapshot: Box::new(snapshot),
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
}

/// The complete set of events serialized by one Device actor in WP7.
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
}

/// Serial Device event loop.
///
/// A stale Client snapshot is ignored before any component call. A current snapshot
/// must reconcile into one complete Server snapshot; validation or component failure
/// drops the lease. Publishing is deliberately non-blocking: a full or closed
/// outbound queue also drops the lease rather than accumulating stale targets.
async fn run_actor(device_id: DeviceId, mut receiver: mpsc::Receiver<DeviceEvent>) {
    let mut current: Option<CurrentLease> = None;
    while let Some(event) = receiver.recv().await {
        match event {
            DeviceEvent::Attach {
                session_id,
                outbound,
                attached,
            } => {
                current = Some(CurrentLease {
                    session_id,
                    outbound,
                });
                let _ = attached.send(());
            }
            DeviceEvent::ClientState {
                state,
                session_id,
                snapshot,
            } => {
                let Some(lease) = current
                    .as_ref()
                    .filter(|lease| lease.session_id == session_id)
                else {
                    continue;
                };
                let Some(snapshot) = state::reconcile(&state, device_id, *snapshot).await else {
                    current = None;
                    continue;
                };
                let envelope = ServerActiveEnvelope {
                    session_id: session_id.to_vec(),
                    body: Some(server_active_envelope::Body::ServerState(snapshot)),
                };
                if lease.outbound.try_send(envelope).is_err() {
                    current = None;
                }
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
