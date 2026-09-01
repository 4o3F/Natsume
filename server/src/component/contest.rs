use snafu::Snafu;

use crate::db::{Database, PersistenceError};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum ContestError {
    #[snafu(display("contest current facts could not be read"))]
    PersistenceFailed,
}

impl From<PersistenceError> for ContestError {
    fn from(error: PersistenceError) -> Self {
        match error {
            PersistenceError::InvalidPersistedData | PersistenceError::OperationFailed => {
                Self::PersistenceFailed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::db::PersistenceError;

    use super::ContestError;

    #[test]
    fn persistence_mapping_covers_every_neutral_variant() {
        for error in [
            PersistenceError::InvalidPersistedData,
            PersistenceError::OperationFailed,
        ] {
            assert_eq!(ContestError::from(error), ContestError::PersistenceFailed);
        }
    }
}
