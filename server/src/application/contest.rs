use snafu::Snafu;

mod account;
mod binding;
mod seat;

pub(crate) use self::{
    account::{AccountFacts, CurrentAccountProjection, NewAccountFacts, list_accounts},
    binding::{BindingFacts, list_bindings},
    seat::{CurrentSeatProjection, SeatFacts, list_seats},
};

/// Redacted persistence boundary shared by Contest-owned adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
#[snafu(module)]
pub(crate) enum ContestPersistenceError {
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
    pub(crate) const fn from_persistence(error: ContestPersistenceError) -> Self {
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
