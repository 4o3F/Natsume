use crate::{application::device::enrollment::EnrollmentError, db::DatabaseError};

pub(crate) mod query;
pub(crate) mod request;
mod row;

impl From<DatabaseError> for EnrollmentError {
    fn from(source: DatabaseError) -> Self {
        match source {
            DatabaseError::InvalidConfiguration
            | DatabaseError::ConnectionFailed
            | DatabaseError::MigrationFailed
            | DatabaseError::TransactionFailed => Self::PersistenceFailed,
        }
    }
}
