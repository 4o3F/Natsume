use crate::db::{Database, TransactionError};

use super::ContestError;

pub(crate) struct AccountFacts {
    account_id: String,
    domjudge_username: String,
    credential_revision: i64,
}

impl AccountFacts {
    pub(super) fn new(
        account_id: String,
        domjudge_username: String,
        credential_revision: i64,
    ) -> Self {
        Self {
            account_id,
            domjudge_username,
            credential_revision,
        }
    }

    pub(crate) fn into_parts(self) -> (String, String, i64) {
        (
            self.account_id,
            self.domjudge_username,
            self.credential_revision,
        )
    }
}

/// Reads the current Account set without secret-storage pointers.
///
/// # Errors
///
/// Returns a redacted [`ContestError`] when persistence fails.
pub(super) async fn list_accounts(database: &Database) -> Result<Vec<AccountFacts>, ContestError> {
    database
        .read(crate::component::contest::db::accounts::list)
        .await
        .map_err(TransactionError::into_error)
        .map_err(ContestError::from)
}
