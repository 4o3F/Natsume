//! Photo-only enrollment and immutable initialization; no daily online workflow.
//! `*_in` component operations run inside the caller's shared database transaction;
//! they do not open connections, start transactions or perform filesystem I/O.
// One mapping defines the wire and persisted spelling of each Photo state/code.
macro_rules! string_enum {
    ($vis:vis enum $name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
        $vis enum $name { $(#[serde(rename = $value)] $variant),+ }
        impl $name {
            pub(super) const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $value),+ }
            }
            pub(super) fn parse(raw: &str) -> Option<Self> {
                match raw { $($value => Some(Self::$variant)),+, _ => None }
            }
        }
    };
}

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

pub(crate) use acceptance::{AcceptanceError, PhotoDecision, ResultImportPreview};
pub(crate) use acceptance::{PendingPhotoPage, PendingPhotoQuery};
pub(crate) use delivery::{DeliveryError, PhotoRecord, PreparedDownload};
pub(crate) use enrollment::{CredentialUse, EnrollmentError, PendingEnrollment, SourceState};
pub(crate) use imports::ResultImportError;
pub(crate) use initialization::{InitializationError, Preparation, SnapshotDownload};
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
    ) -> Result<Self, PhotoStorageError> {
        let results = results_storage::prepare(results).context(ResultsSnafu)?;
        Ok(Self {
            database,
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
