use crate::{application::contest::ContestError, db::DatabaseError};

pub(crate) mod account_mappings;
pub(crate) mod accounts;
pub(crate) mod device_bindings;
pub(crate) mod seats;

impl From<DatabaseError> for ContestError {
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
