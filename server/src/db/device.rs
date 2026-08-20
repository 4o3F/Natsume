use crate::{
    application::device::{DeviceError, DevicePersistenceError},
    db::DatabaseError,
};

pub(crate) mod devices;

impl From<DatabaseError> for DeviceError {
    fn from(source: DatabaseError) -> Self {
        match source {
            DatabaseError::InvalidConfiguration
            | DatabaseError::ConnectionFailed
            | DatabaseError::MigrationFailed
            | DatabaseError::TransactionFailed => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use diesel::{RunQueryDsl, sql_types::Text};

    use crate::{application::device::DeviceError, db::Database};

    pub(crate) async fn test_seed_current_facts(
        database: &Database,
        hardware_id_canary: &str,
    ) -> Result<(), DeviceError> {
        let hardware_id_canary = hardware_id_canary.to_owned();
        database
            .test_write(move |connection| {
                diesel::sql_query(
                    "INSERT INTO devices \
                     (device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES \
                     ('01900000-0000-7000-8000-000000000002', \
                      'machine-hardware-b', 'medium', 'disabled'), \
                     ('01900000-0000-7000-8000-000000000001', ?, 'strong', 'enrolled')",
                )
                .bind::<Text, _>(&hardware_id_canary)
                .execute(connection)
                .map(|_| ())
                .map_err(|_| DeviceError::PersistenceFailed)
            })
            .await
            .map_err(|_| DeviceError::PersistenceFailed)?
    }

    pub(crate) async fn test_set_device_state(
        database: &Database,
        device_id: &str,
        state: &str,
    ) -> Result<(), DeviceError> {
        let device_id = device_id.to_owned();
        let state = state.to_owned();
        database
            .test_write(move |connection| {
                diesel::sql_query("UPDATE devices SET state = ? WHERE device_pk = ?")
                    .bind::<Text, _>(&state)
                    .bind::<Text, _>(&device_id)
                    .execute(connection)
                    .map(|_| ())
                    .map_err(|_| DeviceError::PersistenceFailed)
            })
            .await
            .map_err(|_| DeviceError::PersistenceFailed)?
    }
}

impl From<DatabaseError> for DevicePersistenceError {
    fn from(source: DatabaseError) -> Self {
        match source {
            DatabaseError::InvalidConfiguration
            | DatabaseError::ConnectionFailed
            | DatabaseError::MigrationFailed
            | DatabaseError::TransactionFailed => Self::PersistenceFailed,
        }
    }
}
