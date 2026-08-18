pub(crate) trait DeviceConnectionEvictor: Send + Sync + 'static {
    fn evict_device_connection(&self, device_pk: &str) -> bool;
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct NoLiveDeviceConnections;

#[cfg(test)]
impl DeviceConnectionEvictor for NoLiveDeviceConnections {
    fn evict_device_connection(&self, _device_pk: &str) -> bool {
        false
    }
}
