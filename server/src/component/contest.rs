use snafu::Snafu;

use crate::{
    component::device::DeviceId,
    db::{Database, PersistenceError, TransactionError},
};

mod db;

pub(crate) struct AccountFacts {
    pub(crate) account_id: String,
    pub(crate) domjudge_username: String,
    pub(crate) credential_revision: i64,
}

pub(crate) struct SeatFacts {
    pub(crate) seat_id: String,
    pub(crate) seat_code: String,
}

pub(crate) struct BindingFacts {
    pub(crate) binding: String,
    pub(crate) seat: String,
    pub(crate) device: DeviceId,
}

/// Contest-owned current facts exposed to transport and peer components.
pub(crate) struct ContestComponent {
    database: Database,
}

impl ContestComponent {
    pub(crate) const fn new(database: Database) -> Self {
        Self { database }
    }

    pub(crate) async fn list_seats(&self) -> Result<Vec<SeatFacts>, ContestError> {
        self.database
            .read(db::list_seats)
            .await
            .map_err(TransactionError::into_error)
            .map_err(ContestError::from)
    }

    pub(crate) async fn list_accounts(&self) -> Result<Vec<AccountFacts>, ContestError> {
        self.database
            .read(db::list_accounts)
            .await
            .map_err(TransactionError::into_error)
            .map_err(ContestError::from)
    }

    pub(crate) async fn list_bindings(&self) -> Result<Vec<BindingFacts>, ContestError> {
        self.database
            .read(db::list_bindings)
            .await
            .map_err(TransactionError::into_error)
            .map_err(ContestError::from)
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
    use diesel::connection::SimpleConnection;
    use uuid::Uuid;

    use crate::db::{Database, DatabaseConfig, PersistenceError};

    use super::{ContestComponent, ContestError};

    #[tokio::test]
    async fn current_facts_preserve_contents_and_identifier_order() {
        let root = std::env::temp_dir().join(format!("natsume-contest-test-{}", Uuid::now_v7()));
        std::fs::create_dir(&root).unwrap_or_else(|error| panic!("fixture directory: {error}"));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(root.join("db"), true))
            .await
            .unwrap_or_else(|error| panic!("fixture database: {error}"));
        database
            .write(|transaction| {
                transaction
                    .connection()
                    .batch_execute(
                        "INSERT INTO seats VALUES ('seat-b', 'A'), ('seat-a', 'Z');
                 INSERT INTO accounts VALUES ('account-b', 'alice', 3), ('account-a', 'zoe', 2);
                 INSERT INTO devices VALUES
                   ('01900000-0000-7000-8000-000000000002', 'machine-b', 'strong', 'enabled', 1),
                   ('01900000-0000-7000-8000-000000000001', 'machine-a', 'strong', 'enabled', 1);
                 INSERT INTO device_bindings VALUES
                   ('binding-a', '01900000-0000-7000-8000-000000000001', 'seat-b'),
                   ('binding-b', '01900000-0000-7000-8000-000000000002', 'seat-a');",
                    )
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .unwrap_or_else(|error| panic!("fixture rows: {error:?}"));
        let contest = ContestComponent::new(database.clone());

        let seats = contest
            .list_seats()
            .await
            .unwrap_or_else(|error| panic!("Seat query: {error}"));
        assert_eq!(
            seats
                .iter()
                .map(|seat| (seat.seat_id.as_str(), seat.seat_code.as_str()))
                .collect::<Vec<_>>(),
            [("seat-a", "Z"), ("seat-b", "A")]
        );
        let accounts = contest
            .list_accounts()
            .await
            .unwrap_or_else(|error| panic!("Account query: {error}"));
        assert_eq!(
            accounts
                .iter()
                .map(|account| (
                    account.account_id.as_str(),
                    account.domjudge_username.as_str(),
                    account.credential_revision
                ))
                .collect::<Vec<_>>(),
            [("account-a", "zoe", 2), ("account-b", "alice", 3)]
        );
        let bindings = contest
            .list_bindings()
            .await
            .unwrap_or_else(|error| panic!("Binding query: {error}"));
        assert_eq!(
            bindings
                .iter()
                .map(|binding| (
                    binding.binding.as_str(),
                    binding.seat.as_str(),
                    binding.device.as_text()
                ))
                .collect::<Vec<_>>(),
            [
                (
                    "binding-b",
                    "seat-a",
                    "01900000-0000-7000-8000-000000000002".to_owned()
                ),
                (
                    "binding-a",
                    "seat-b",
                    "01900000-0000-7000-8000-000000000001".to_owned()
                ),
            ]
        );
        drop(contest);
        drop(database);
        std::fs::remove_dir_all(root).unwrap_or_else(|error| panic!("fixture cleanup: {error}"));
    }

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
