//! Photo enrollment, initialization and signed results for the offline Client workflow.
mod acceptance;
mod db;
mod delivery;
mod enrollment;
mod imports;
mod initialization;
pub(crate) mod limits;
mod results_storage;
mod roster;
mod storage;
mod transfer;
mod validation;

pub(crate) use acceptance::PendingPhotoQuery;
pub(crate) use acceptance::{AcceptanceError, PhotoDecision};
pub(crate) use delivery::{DeliveryError, PreparedDownload};
pub(crate) use enrollment::{CredentialUse, EnrollmentError, PendingEnrollment, SourceState};
pub(crate) use imports::ResultImportError;
pub(crate) use initialization::InitializationError;
pub(crate) use roster::PhotoRoster;
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
    contest: Arc<super::contest::ContestComponent>,
    lifecycle: crate::lifecycle::ServerLifecycle,
    storage: storage::PhotoStore,
    results: results_storage::ResultsStore,
    export_slots: Arc<Semaphore>,
    photo_download_slots: Arc<Semaphore>,
    validation_wake: Arc<tokio::sync::Notify>,
    pairing_attempts: Mutex<super::pairing::PairingBudget>,
    snapshot_build_slots: Arc<Semaphore>,
    snapshot_download_slots: Arc<Semaphore>,
}

impl PhotoComponent {
    pub(crate) fn load(
        database: Database,
        initializations: &Path,
        results: &Path,
        contest: Arc<super::contest::ContestComponent>,
        lifecycle: crate::lifecycle::ServerLifecycle,
    ) -> Result<Self, PhotoStorageError> {
        let results = results_storage::prepare(results).context(ResultsSnafu)?;
        Ok(Self {
            database,
            contest,
            lifecycle,
            storage: storage::PhotoStore::open(initializations).context(InitializationSnafu)?,
            results,
            export_slots: Arc::new(Semaphore::new(1)),
            photo_download_slots: Arc::new(Semaphore::new(2)),
            validation_wake: Arc::new(tokio::sync::Notify::new()),
            pairing_attempts: Mutex::new(super::pairing::PairingBudget::default()),
            snapshot_build_slots: Arc::new(Semaphore::new(1)),
            snapshot_download_slots: Arc::new(Semaphore::new(2)),
        })
    }
}

/// Typed boundary for Photo operations; submodules retain their own failures.
#[derive(Debug, Snafu)]
pub(crate) enum PhotoError {
    #[snafu(context(false), display("Photo enrollment failed"))]
    Enrollment { source: EnrollmentError },
    #[snafu(context(false), display("Photo initialization failed"))]
    Initialization { source: InitializationError },
    #[snafu(context(false), display("Photo result import failed"))]
    Import { source: ResultImportError },
    #[snafu(context(false), display("Photo acceptance failed"))]
    Acceptance { source: AcceptanceError },
    #[snafu(context(false), display("Photo delivery failed"))]
    Delivery { source: DeliveryError },
}

fn load_roster(
    tx: &mut crate::db::Transaction<'_>,
) -> Result<PhotoRoster, super::contest::ContestError> {
    super::contest::ContestComponent::photo_roster_in(tx).map(PhotoRoster::from)
}
