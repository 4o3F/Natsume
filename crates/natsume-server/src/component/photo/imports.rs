//! Bounded upload admission and durable upload facts, separate from package validation.
use super::{PhotoComponent, db, results_storage};
use crate::db::TransactionError;
use snafu::{ResultExt as _, Snafu, ensure};
use tokio::io::AsyncRead;
use uuid::Uuid;

string_enum! {
    pub(crate) enum ResultImportState {
        Uploading => "uploading", Processing => "processing", Ready => "ready",
        Committed => "committed", Failed => "failed",
    }
}

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct ResultImportJob {
    pub import_id: Uuid,
    pub state: ResultImportState,
    #[specta(type = specta_typescript::Number)]
    pub uploaded_bytes: i64,
    #[specta(type = specta_typescript::Number)]
    pub verified_images: i64,
    pub failure_code: Option<ResultImportFailure>,
}

string_enum! {
    pub(crate) enum ResultImportFailure {
        Limit => "limit", Input => "input", Storage => "storage", Archive => "archive",
        Signature => "signature", Authorization => "authorization", Image => "image",
        Conflict => "conflict", Interrupted => "interrupted",
    }
}

#[derive(Debug, Snafu)]
pub(crate) enum ResultImportError {
    #[snafu(display("Photo upload capacity is exhausted"))]
    Busy,
    #[snafu(display("Photo import does not exist"))]
    NotFound,
    #[snafu(display("Photo upload exceeds its resource limit"))]
    Limit,
    #[snafu(display("Photo upload is empty"))]
    Empty,
    #[snafu(display("Photo upload input failed"))]
    Input { source: std::io::Error },
    #[snafu(display("Photo upload file operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("Photo upload storage failed"))]
    Storage {
        source: results_storage::ResultsStorageError,
    },
    #[snafu(display("Photo import persistence failed"))]
    Persistence { source: db::PersistenceError },
    #[snafu(display("Photo upload worker failed"))]
    Worker {
        #[snafu(source(from(tokio::task::JoinError, crate::task::TaskFailure::from)))]
        source: crate::task::TaskFailure,
    },
}

impl From<&ResultImportError> for ResultImportFailure {
    fn from(error: &ResultImportError) -> Self {
        match error {
            ResultImportError::Limit => Self::Limit,
            ResultImportError::Input { .. } | ResultImportError::Empty => Self::Input,
            _ => Self::Storage,
        }
    }
}

impl PhotoComponent {
    pub(crate) async fn read_import(&self, id: Uuid) -> Result<ResultImportJob, ResultImportError> {
        self.database
            .read(move |tx| db::jobs::read(tx, id))
            .await
            .map_err(TransactionError::into_error)
            .context(PersistenceSnafu)?
            .ok_or_else(|| NotFoundSnafu.build())
    }

    pub(crate) async fn upload_results<R>(
        &self,
        input: R,
    ) -> Result<ResultImportJob, ResultImportError>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let id = Uuid::now_v7();
        let admitted = self
            .database
            .write(move |tx| {
                if db::jobs::active_count(tx)? != 0 {
                    return Ok::<_, db::PersistenceError>(false);
                }
                db::jobs::begin_upload(tx, id)?;
                Ok(true)
            })
            .await
            .map_err(TransactionError::into_error)
            .context(PersistenceSnafu)?;
        ensure!(admitted, BusySnafu);
        let database = self.database.clone();
        let store = self.results.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            // The durable nonterminal job reserves admission until physical IO ends.
            let mut owned = false;
            let mut total = 0_u64;
            let result = (|| {
                let mut files = store.create_upload(id).context(StorageSnafu)?;
                owned = true;
                files.directory.sync_all().context(IoSnafu)?;
                let mut input =
                    tokio_util::io::SyncIoBridge::new_with_handle(input, runtime.clone());
                super::transfer::copy_verified(
                    &mut input,
                    &mut files.file,
                    super::transfer::Expectation::Upload {
                        limit: natsume_photo_protocol::results::PACKAGE_LIMIT,
                    },
                    &mut total,
                    || true,
                )?;
                ensure!(total != 0, EmptySnafu);
                files.file.sync_all().context(IoSnafu)?;
                files.directory.sync_all().context(IoSnafu)?;
                runtime
                    .block_on(database.write(move |tx| db::jobs::finish_upload(tx, id, total)))
                    .map_err(TransactionError::into_error)
                    .context(PersistenceSnafu)?;
                Ok::<_, ResultImportError>(())
            })();
            if let Err(error) = result {
                // Only unlink a name exclusively created by this admitted operation.
                if owned && let Err(cleanup) = store.remove_upload(id) {
                    tracing::error!(%cleanup, "Photo partial upload cleanup failed");
                    // Leave the durable uploading fact to block further admission.
                    return Err(error);
                }
                let failure = ResultImportFailure::from(&error);
                if let Err(persistence) = runtime.block_on(database.write(move |tx| {
                    db::jobs::mark_failed(
                        tx,
                        id,
                        failure,
                        Some(total.min(natsume_photo_protocol::results::PACKAGE_LIMIT)),
                    )
                })) {
                    let persistence = persistence.into_error();
                    tracing::error!(%persistence, "Photo upload failure could not be recorded");
                }
                return Err(error);
            }
            Ok(())
        })
        .await
        .context(WorkerSnafu)??;
        self.validation_wake.notify_one();
        self.read_import(id).await
    }
}

impl From<super::transfer::TransferError> for ResultImportError {
    fn from(error: super::transfer::TransferError) -> Self {
        use super::transfer::TransferError as Error;
        match error {
            Error::Read { source } => Self::Input { source },
            Error::Write { source } => Self::Io { source },
            Error::Limit | Error::Integrity => Self::Limit,
            Error::Interrupted => Self::Input {
                source: std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "Photo upload interrupted",
                ),
            },
        }
    }
}
