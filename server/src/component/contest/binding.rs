use crate::db::Database;

use super::{super::lifecycle::DeviceId, ContestError};

pub(crate) struct BindingFacts {
    binding: String,
    seat: String,
    device: DeviceId,
}

impl BindingFacts {
    pub(crate) fn new(binding_id: String, seat_id: String, device_id: DeviceId) -> Self {
        Self {
            binding: binding_id,
            seat: seat_id,
            device: device_id,
        }
    }

    pub(crate) fn into_parts(self) -> (String, String, DeviceId) {
        (self.binding, self.seat, self.device)
    }
}

/// Reads the current Seat-to-Device Binding set in Seat-key order.
///
/// # Errors
///
/// Returns a redacted [`ContestError`] when persistence fails.
pub(crate) async fn list_bindings(database: &Database) -> Result<Vec<BindingFacts>, ContestError> {
    database
        .read(crate::component::contest::db::device_bindings::list)
        .await
        .map_err(ContestError::from_persistence)
}
