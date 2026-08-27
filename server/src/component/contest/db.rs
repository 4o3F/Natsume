use crate::{component::contest::ContestPersistenceError, db::DatabaseError};

pub(crate) mod accounts;
pub(crate) mod device_bindings;
pub(crate) mod seats;

impl From<DatabaseError> for ContestPersistenceError {
    fn from(error: DatabaseError) -> Self {
        match error {
            DatabaseError::InvalidConfiguration
            | DatabaseError::ConnectionFailed
            | DatabaseError::MigrationFailed
            | DatabaseError::TransactionFailed => Self::PersistenceFailed,
        }
    }
}
