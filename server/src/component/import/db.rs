use uuid::Uuid;

use crate::{component::import::ImportError, db::DatabaseError};

pub(crate) mod account_mappings;
pub(crate) mod accounts;
pub(crate) mod pending_import_candidate;
pub(crate) mod query;
pub(crate) mod seats;
pub(crate) mod server_vault_records;

fn canonical_uuid_v7(value: &str) -> Result<Uuid, ImportError> {
    let parsed = Uuid::parse_str(value).map_err(|_| ImportError::PersistenceFailure)?;
    if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != value {
        return Err(ImportError::PersistenceFailure);
    }
    Ok(parsed)
}

impl From<DatabaseError> for ImportError {
    fn from(source: DatabaseError) -> Self {
        match source {
            DatabaseError::InvalidConfiguration
            | DatabaseError::ConnectionFailed
            | DatabaseError::MigrationFailed
            | DatabaseError::TransactionFailed => Self::PersistenceFailure,
        }
    }
}
