use crate::{component::contest::ContestPersistenceError, db::DatabaseError};

pub(super) mod accounts;
pub(super) mod device_bindings;
pub(super) mod seats;

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
