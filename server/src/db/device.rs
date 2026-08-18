use crate::{
    application::device::{DeviceError, DevicePersistenceError},
    db::DatabaseError,
};

pub(crate) mod certificates;
pub(crate) mod devices;
pub(crate) mod enrollment;
pub(crate) mod query;
pub(crate) mod tokens;

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

#[cfg(test)]
pub(crate) mod tests;
