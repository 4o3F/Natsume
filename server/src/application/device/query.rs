use crate::db::{self, Database};

use super::{DeviceError, DeviceFacts};

/// Reads the current Device set without Machine Hardware IDs.
///
/// # Errors
///
/// Returns a redacted [`DeviceError`] when persistence fails or a persisted
/// vocabulary value is outside its frozen set.
pub(crate) async fn list_devices(database: &Database) -> Result<Vec<DeviceFacts>, DeviceError> {
    database
        .read(db::device::devices::list)
        .await
        .map_err(DeviceError::from_persistence)
}
