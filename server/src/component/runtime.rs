mod db;

use snafu::Snafu;
use url::Url;

use crate::db::{Database, PersistenceError, TransactionError};

pub(crate) struct RuntimeConfigComponent {
    database: Database,
}

impl RuntimeConfigComponent {
    pub(crate) const fn new(database: Database) -> Self {
        Self { database }
    }

    /// Reads the durable `DOMjudge` origin without requiring it to be configured.
    pub(crate) async fn read_current(&self) -> Result<Option<String>, RuntimeConfigError> {
        self.database
            .read(|transaction| {
                let rows = db::read_all(transaction)?;
                match rows.as_slice() {
                    [] => Ok(None),
                    [(1, origin)] if is_canonical_https_origin(origin) => Ok(Some(origin.clone())),
                    _ => Err(RuntimeConfigError::InvalidPersistedFacts),
                }
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn materialize(&self) -> Result<String, RuntimeConfigError> {
        self.read_current()
            .await?
            .ok_or(RuntimeConfigError::MissingConfiguration)
    }
}

pub(crate) fn is_canonical_https_origin(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.host().is_some()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url.origin().ascii_serialization() == value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum RuntimeConfigError {
    #[snafu(display("Runtime Config has not been configured"))]
    MissingConfiguration,
    #[snafu(display("persisted Runtime Config facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("Runtime Config persistence failed"))]
    PersistenceFailed,
}

impl From<PersistenceError> for RuntimeConfigError {
    fn from(error: PersistenceError) -> Self {
        match error {
            PersistenceError::InvalidPersistedData => Self::InvalidPersistedFacts,
            PersistenceError::OperationFailed => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
mod tests;
