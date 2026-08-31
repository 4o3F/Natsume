use snafu::Snafu;

use crate::db::Database;

mod account;
mod binding;
mod db;
mod seat;

pub(crate) use self::{
    account::{AccountFacts, CurrentAccountProjection, NewAccountFacts},
    binding::BindingFacts,
    seat::{CurrentSeatProjection, SeatFacts},
};

/// Contest-owned current facts exposed to transport and peer components.
pub(crate) struct ContestComponent {
    database: Database,
}

impl ContestComponent {
    pub(crate) const fn new(database: Database) -> Self {
        Self { database }
    }

    pub(crate) async fn list_seats(&self) -> Result<Vec<SeatFacts>, ContestError> {
        seat::list_seats(&self.database).await
    }

    pub(crate) async fn list_accounts(&self) -> Result<Vec<AccountFacts>, ContestError> {
        account::list_accounts(&self.database).await
    }

    pub(crate) async fn list_bindings(&self) -> Result<Vec<BindingFacts>, ContestError> {
        binding::list_bindings(&self.database).await
    }
}

/// Redacted persistence boundary shared by Contest-owned adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
#[snafu(module)]
enum ContestPersistenceError {
    #[snafu(display("persisted contest facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("contest persistence failed"))]
    PersistenceFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum ContestError {
    #[snafu(display("contest current facts could not be read"))]
    PersistenceFailed,
}

impl ContestError {
    const fn from_persistence(error: ContestPersistenceError) -> Self {
        match error {
            ContestPersistenceError::InvalidPersistedFacts
            | ContestPersistenceError::PersistenceFailed => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContestError, ContestPersistenceError};

    #[test]
    fn persistence_mapping_covers_every_neutral_variant() {
        for error in [
            ContestPersistenceError::InvalidPersistedFacts,
            ContestPersistenceError::PersistenceFailed,
        ] {
            assert_eq!(
                ContestError::from_persistence(error),
                ContestError::PersistenceFailed
            );
        }
    }
}
