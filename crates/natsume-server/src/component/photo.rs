//! Photo-only enrollment and immutable initialization; no daily online workflow.
mod db;
mod enrollment;
mod initialization;
mod results_storage;
mod storage;

pub(crate) use enrollment::{CredentialUse, EnrollmentError, PendingEnrollment, SourceState};
pub(crate) use initialization::{InitializationError, Preparation, SnapshotDownload};

use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;

use crate::db::Database;
use snafu::{ResultExt as _, Snafu};

#[derive(Debug, Snafu)]
pub(crate) enum PhotoStorageError {
    #[snafu(display("Photo initialization storage startup failed"))]
    Initialization { source: InitializationError },
    #[snafu(display("Photo results storage startup failed"))]
    Results {
        source: results_storage::ResultsStorageError,
    },
    #[snafu(display("Photo storage startup task failed"))]
    Worker { source: tokio::task::JoinError },
}

pub(crate) struct PhotoComponent {
    database: Database,
    storage: storage::PhotoStore,
    pairing_attempts: Mutex<super::pairing::PairingBudget>,
    snapshot_work: Arc<Semaphore>,
    download_work: Arc<Semaphore>,
}

impl PhotoComponent {
    pub(crate) fn load(
        database: Database,
        initializations: &Path,
        results: &Path,
    ) -> Result<Self, PhotoStorageError> {
        results_storage::prepare(results).context(ResultsSnafu)?;
        Self::new(database, initializations).context(InitializationSnafu)
    }

    pub(crate) fn new(database: Database, directory: &Path) -> Result<Self, InitializationError> {
        Ok(Self {
            database,
            storage: storage::PhotoStore::open(directory)?,
            pairing_attempts: Mutex::new(super::pairing::PairingBudget::default()),
            snapshot_work: Arc::new(Semaphore::new(1)),
            download_work: Arc::new(Semaphore::new(2)),
        })
    }
}
