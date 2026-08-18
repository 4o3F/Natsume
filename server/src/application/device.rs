mod connections;
pub(crate) mod credentials;
pub(crate) mod enrollment;
mod lifecycle;
mod query;
mod types;

pub(crate) use self::{
    connections::DeviceConnectionEvictor,
    lifecycle::{DeviceLifecycleAction, DeviceLifecycleFacts, disable_device, revoke_device},
    query::list_devices,
    types::{
        DeviceByHardwareProjection, DeviceError, DeviceFacts, DeviceId, DevicePersistenceError,
        DeviceState, HardwareIdentityQuality,
    },
};

#[cfg(test)]
pub(crate) use self::connections::NoLiveDeviceConnections;
