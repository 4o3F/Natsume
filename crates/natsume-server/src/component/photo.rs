//! Photo-only enrollment and immutable initialization; no daily online workflow.
mod db;
mod enrollment;
mod initialization;

pub(crate) use enrollment::{CredentialUse, EnrollmentError, PendingEnrollment, SourceState};
pub(crate) use initialization::InitializationError;

use std::{collections::VecDeque, net::IpAddr, path::PathBuf, sync::Arc};
use tokio::{
    sync::{Mutex, Semaphore},
    time::Instant,
};

use crate::db::Database;

pub(crate) struct PhotoComponent {
    database: Database,
    directory: PathBuf,
    pairing_attempts: Mutex<VecDeque<(Instant, IpAddr)>>,
    snapshot_work: Arc<Semaphore>,
    download_work: Arc<Semaphore>,
}

impl PhotoComponent {
    pub(crate) fn new(database: Database, directory: PathBuf) -> Self {
        Self {
            database,
            directory,
            pairing_attempts: Mutex::new(VecDeque::new()),
            snapshot_work: Arc::new(Semaphore::new(1)),
            download_work: Arc::new(Semaphore::new(2)),
        }
    }
}
