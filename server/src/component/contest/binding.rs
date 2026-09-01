use crate::db::{Database, TransactionError};

use super::{super::device::DeviceId, ContestError};

pub(crate) struct BindingFacts {
    binding: String,
    seat: String,
    device: DeviceId,
}

impl BindingFacts {
    pub(super) fn new(binding_id: String, seat_id: String, device_id: DeviceId) -> Self {
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
pub(super) async fn list_bindings(database: &Database) -> Result<Vec<BindingFacts>, ContestError> {
    database
        .read(crate::component::contest::db::device_bindings::list)
        .await
        .map_err(TransactionError::into_error)
        .map_err(ContestError::from)
}
