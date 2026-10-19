use std::{path::PathBuf, sync::Arc};

use snafu::Snafu;
use tokio::sync::Semaphore;

use crate::{
    component::device::DeviceId,
    db::{Database, PersistenceError, TransactionError},
};

mod db;
mod export;
mod logo_cache;
mod logos;
mod roster;

pub(crate) use export::ExportError;
pub(crate) use logos::{LogoObservation, LogoStatus};
pub(crate) use roster::OrganizationDetails;

pub(crate) struct TeamDetails {
    pub(crate) seat_id: String,
    pub(crate) seat_code: String,
    pub(crate) organization_id: i64,
    pub(crate) team_name_zh: String,
    pub(crate) team_name_en: String,
    pub(crate) school_name_zh: String,
    pub(crate) school_name_en: String,
}

pub(crate) struct AccountFacts {
    pub(crate) account_id: String,
    pub(crate) domjudge_username: String,
    pub(crate) credential_revision: i64,
    pub(crate) team: Option<TeamDetails>,
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
    vault: Arc<crate::vault::VaultSession>,
    logo_directory: PathBuf,
    image_work: Arc<Semaphore>,
    logo_validation: Arc<logo_cache::LogoValidationCache>,
    export_work: Arc<Semaphore>,
}

impl ContestComponent {
    pub(crate) fn new(
        database: Database,
        vault: Arc<crate::vault::VaultSession>,
        logo_directory: PathBuf,
    ) -> Self {
        Self {
            database,
            vault,
            logo_directory,
            image_work: Arc::new(Semaphore::new(4)),
            logo_validation: Arc::new(logo_cache::LogoValidationCache::default()),
            export_work: Arc::new(Semaphore::new(1)),
        }
    }

    pub(crate) async fn list_organizations(
        &self,
    ) -> Result<Vec<OrganizationDetails>, ContestError> {
        self.database
            .read(db::list_organizations)
            .await
            .map_err(TransactionError::into_error)
            .map_err(ContestError::from)
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
    use std::os::unix::fs::PermissionsExt as _;
    use uuid::Uuid;

    use crate::db::{Database, DatabaseConfig, PersistenceError};

    use super::{ContestComponent, ContestError};

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn current_facts_preserve_contents_and_identifier_order() {
        let root = std::env::temp_dir().join(format!("natsume-contest-test-{}", Uuid::now_v7()));
        std::fs::create_dir(&root).unwrap_or_else(|error| panic!("fixture directory: {error}"));
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("permissions: {error}"));
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
                 INSERT INTO organizations VALUES (1, '示例大学', '示例大学', 'Example University', 'CHN');
                 INSERT INTO teams VALUES ('account-a', 1, '队伍', 'Team', 'participant');
                 INSERT INTO account_mappings VALUES ('seat-a', 'account-a');
                 INSERT INTO devices (device_id, machine_hardware_id, evidence_quality, state, created_at_unix_ms) VALUES
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
        crate::vault::ensure_master_key(&root.join("key"))
            .unwrap_or_else(|error| panic!("vault: {error}"));
        let vault =
            crate::vault::load(&root.join("key")).unwrap_or_else(|error| panic!("vault: {error}"));
        let contest = ContestComponent::new(
            database.clone(),
            std::sync::Arc::new(vault),
            root.join("logos"),
        );

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
        let team = accounts[0]
            .team
            .as_ref()
            .unwrap_or_else(|| panic!("missing team profile"));
        assert_eq!(
            (team.seat_id.as_str(), team.seat_code.as_str()),
            ("seat-a", "Z")
        );
        assert_eq!(team.organization_id, 1);
        assert_eq!(team.team_name_zh, "队伍");
        assert_eq!(team.school_name_en, "Example University");
        assert!(accounts[1].team.is_none());
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
