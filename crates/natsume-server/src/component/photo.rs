//! Photo-only enrollment and immutable initialization; no daily online workflow.
mod acceptance;
mod db;
mod delivery;
mod enrollment;
mod imports;
mod initialization;
mod results_storage;
mod storage;
mod validation;

pub(crate) use acceptance::{AcceptanceError, ImportPreview, PhotoDecision};
pub(crate) use acceptance::{PendingPhotoPage, PendingPhotoQuery};
pub(crate) use delivery::{DeliveryError, PhotoPage, PhotoQuery, PreparedDownload};
pub(crate) use enrollment::{CredentialUse, EnrollmentError, PendingEnrollment, SourceState};
pub(crate) use imports::{ImportError, ImportJob, ImportState};
pub(crate) use initialization::{InitializationError, Preparation, SnapshotDownload};
pub(crate) use validation::ValidationError;

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
    Worker {
        #[snafu(source(from(tokio::task::JoinError, crate::task::TaskFailure::from)))]
        source: crate::task::TaskFailure,
    },
}

pub(crate) struct PhotoComponent {
    database: Database,
    storage: storage::PhotoStore,
    results: results_storage::ResultsStore,
    upload_work: Arc<Semaphore>,
    export_work: Arc<Semaphore>,
    result_download_work: Arc<Semaphore>,
    validation_wake: Arc<tokio::sync::Notify>,
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
        let results = results_storage::prepare(results).context(ResultsSnafu)?;
        Ok(Self {
            database,
            storage: storage::PhotoStore::open(initializations).context(InitializationSnafu)?,
            results,
            upload_work: Arc::new(Semaphore::new(1)),
            export_work: Arc::new(Semaphore::new(1)),
            result_download_work: Arc::new(Semaphore::new(2)),
            validation_wake: Arc::new(tokio::sync::Notify::new()),
            pairing_attempts: Mutex::new(super::pairing::PairingBudget::default()),
            snapshot_work: Arc::new(Semaphore::new(1)),
            download_work: Arc::new(Semaphore::new(2)),
        })
    }
}
