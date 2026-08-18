use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use tokio::sync::{Notify, watch};

use crate::application::{command::DeviceCommandDispatchNotifier, device::DeviceConnectionEvictor};

#[derive(Clone)]
pub(crate) struct DeviceConnectionRegistry {
    connections: Arc<Mutex<HashMap<String, ConnectionHandle>>>,
}

impl DeviceConnectionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn evict(&self, device_pk: &str) -> bool {
        let handle = self.lock_connections().remove(device_pk);
        let Some(handle) = handle else {
            return false;
        };
        let _send_result = handle.eviction.send(true);
        true
    }

    pub(super) fn notify_dispatch(&self, device_pk: &str) -> bool {
        let connections = self.lock_connections();
        let Some(handle) = connections.get(device_pk) else {
            return false;
        };
        handle.dispatch.notify_one();
        true
    }

    pub(super) fn register(
        &self,
        device_pk: String,
        connection_epoch: u64,
    ) -> RegisteredConnection {
        let (eviction, receiver) = watch::channel(false);
        let dispatch = Arc::new(Notify::new());
        let previous = self.lock_connections().insert(
            device_pk.clone(),
            ConnectionHandle {
                eviction,
                connection_epoch,
                dispatch: dispatch.clone(),
            },
        );
        let displaced = match previous {
            Some(previous) => {
                let _send_result = previous.eviction.send(true);
                DisplacedConnection::Evicted
            }
            None => DisplacedConnection::None,
        };
        RegisteredConnection {
            registration: ConnectionRegistration {
                registry: self.clone(),
                device_pk,
                connection_epoch,
            },
            eviction: receiver,
            dispatch,
            displaced,
        }
    }

    fn remove_if_current(&self, device_pk: &str, connection_epoch: u64) {
        let mut connections = self.lock_connections();
        if connections
            .get(device_pk)
            .is_some_and(|handle| handle.connection_epoch == connection_epoch)
        {
            connections.remove(device_pk);
        }
    }

    fn lock_connections(&self) -> MutexGuard<'_, HashMap<String, ConnectionHandle>> {
        self.connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl DeviceConnectionEvictor for DeviceConnectionRegistry {
    fn evict_device_connection(&self, device_pk: &str) -> bool {
        self.evict(device_pk)
    }
}

impl DeviceCommandDispatchNotifier for DeviceConnectionRegistry {
    fn notify_command_dispatch(&self, device_pk: &str) {
        let _notified = self.notify_dispatch(device_pk);
    }
}

pub(super) struct RegisteredConnection {
    pub(super) registration: ConnectionRegistration,
    pub(super) eviction: watch::Receiver<bool>,
    pub(super) dispatch: Arc<Notify>,
    pub(super) displaced: DisplacedConnection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DisplacedConnection {
    None,
    Evicted,
}

struct ConnectionHandle {
    eviction: watch::Sender<bool>,
    connection_epoch: u64,
    dispatch: Arc<Notify>,
}

pub(super) struct ConnectionRegistration {
    registry: DeviceConnectionRegistry,
    device_pk: String,
    connection_epoch: u64,
}

impl Drop for ConnectionRegistration {
    fn drop(&mut self) {
        self.registry
            .remove_if_current(&self.device_pk, self.connection_epoch);
    }
}
