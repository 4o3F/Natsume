use uuid::Uuid;

use crate::db::Database;

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

pub(crate) struct NewAccountFacts {
    account_id: Uuid,
    domjudge_username: String,
}

impl NewAccountFacts {
    pub(crate) fn new(domjudge_username: String) -> Self {
        Self {
            account_id: Uuid::now_v7(),
            domjudge_username,
        }
    }

    #[must_use]
    pub(crate) const fn account_id(&self) -> Uuid {
        self.account_id
    }

    #[must_use]
    pub(crate) fn domjudge_username(&self) -> &str {
        &self.domjudge_username
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentAccountProjection {
    account_id: String,
    domjudge_username: String,
    credential_revision: i64,
}

impl CurrentAccountProjection {
    pub(crate) fn new(
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

    #[must_use]
    pub(crate) fn account_id(&self) -> &str {
        &self.account_id
    }

    #[must_use]
    pub(crate) fn domjudge_username(&self) -> &str {
        &self.domjudge_username
    }

    #[must_use]
    pub(crate) const fn credential_revision(&self) -> i64 {
        self.credential_revision
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
        .map_err(ContestError::from_persistence)
}
