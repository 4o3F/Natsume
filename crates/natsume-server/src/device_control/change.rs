use crate::component::device::DeviceId;

#[must_use = "publish the committed target change through DeviceControl::publish"]
#[derive(Debug)]
pub(crate) struct TargetChange {
    scope: TargetScope,
}

#[derive(Debug)]
pub(super) enum TargetScope {
    Device(DeviceId),
    Devices(Vec<DeviceId>),
    AllConnected,
}

impl TargetChange {
    pub(crate) const fn device(device_id: DeviceId) -> Self {
        Self {
            scope: TargetScope::Device(device_id),
        }
    }

    pub(crate) fn devices(device_ids: Vec<DeviceId>) -> Self {
        Self {
            scope: TargetScope::Devices(device_ids),
        }
    }

    pub(crate) const fn all_connected() -> Self {
        Self {
            scope: TargetScope::AllConnected,
        }
    }

    pub(super) fn into_scope(self) -> TargetScope {
        self.scope
    }
}
