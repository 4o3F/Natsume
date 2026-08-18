use crate::db::{self, Database};

use super::{super::device::DeviceId, ContestError};

pub(crate) struct BindingFacts {
    seat_id: String,
    device_id: DeviceId,
    binding_revision: i64,
}

impl BindingFacts {
    pub(crate) fn new(seat_id: String, device_id: DeviceId, binding_revision: i64) -> Self {
        Self {
            seat_id,
            device_id,
            binding_revision,
        }
    }

    pub(crate) fn into_parts(self) -> (String, DeviceId, i64) {
        (self.seat_id, self.device_id, self.binding_revision)
    }
}

/// Reads the current Seat-to-Device Binding set in Seat-key order.
///
/// # Errors
///
/// Returns a redacted [`ContestError`] when persistence fails.
pub(crate) async fn list_bindings(database: &Database) -> Result<Vec<BindingFacts>, ContestError> {
    database
        .read(db::contest::device_bindings::list)
        .await
        .map_err(ContestError::from_persistence)
}
