mod db;

use snafu::Snafu;
use url::Url;

use crate::db::{Database, PersistenceError, Transaction, TransactionError};

pub(crate) struct RuntimeConfigComponent {
    database: Database,
}

impl RuntimeConfigComponent {
    pub(crate) const fn new(database: Database) -> Self {
        Self { database }
    }

    /// Applies the validated deployment origin within the caller's transaction.
    pub(crate) fn apply_deployment_config(
        transaction: &mut Transaction<'_>,
        origin: &str,
    ) -> Result<(), RuntimeConfigError> {
        match db::read_all(transaction)?.as_slice() {
            [(1, current)] if current == origin => Ok(()),
            [] | [(1, _)] => db::upsert(transaction, origin).map_err(RuntimeConfigError::from),
            _ => Err(RuntimeConfigError::InvalidPersistedFacts {
                source: crate::db::PersistenceError::invalid("persisted facts"),
            }),
        }
    }

    /// Reads the durable `DOMjudge` origin without requiring it to be configured.
    pub(crate) async fn read_current(&self) -> Result<Option<String>, RuntimeConfigError> {
        self.database
            .read(|transaction| {
                let rows = db::read_all(transaction)?;
                match rows.as_slice() {
                    [] => Ok(None),
                    [(1, origin)] if is_canonical_https_origin(origin) => Ok(Some(origin.clone())),
                    _ => Err(RuntimeConfigError::InvalidPersistedFacts {
                        source: crate::db::PersistenceError::invalid("persisted facts"),
                    }),
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

#[derive(Debug, Snafu)]
pub(crate) enum RuntimeConfigError {
    #[snafu(display("Runtime Config has not been configured"))]
    MissingConfiguration,
    #[snafu(display("persisted Runtime Config facts are invalid"))]
    InvalidPersistedFacts { source: PersistenceError },
    #[snafu(display("Runtime Config persistence failed"))]
    PersistenceFailed { source: PersistenceError },
}

impl RuntimeConfigError {
    pub(crate) fn is_transient(&self) -> bool {
        matches!(self, Self::PersistenceFailed { source } if source.is_transient())
    }
}

impl From<PersistenceError> for RuntimeConfigError {
    fn from(source: PersistenceError) -> Self {
        match source {
            source if source.is_invalid_data() => Self::InvalidPersistedFacts { source },
            source => Self::PersistenceFailed { source },
        }
    }
}

#[cfg(test)]
mod tests;
