use uuid::Uuid;

use crate::db::PersistenceError;

pub(super) mod account_mappings;
pub(super) mod accounts;
pub(super) mod pending_import_candidate;
pub(super) mod query;
pub(super) mod seats;
pub(super) mod server_vault_records;

fn canonical_uuid_v7(value: &str) -> Result<Uuid, PersistenceError> {
    let parsed = Uuid::parse_str(value).map_err(|_| PersistenceError::InvalidPersistedData)?;
    if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != value {
        return Err(PersistenceError::InvalidPersistedData);
    }
    Ok(parsed)
}
