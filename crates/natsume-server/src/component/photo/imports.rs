//! Bounded upload admission and durable upload facts, separate from package validation.
use super::{PhotoComponent, db, results_storage};
use crate::db::TransactionError;
use snafu::{ResultExt as _, Snafu, ensure};
use std::io::{Read as _, Write as _};
use tokio::io::AsyncRead;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportState {
    Uploading,
    Uploaded,
    Validating,
    Ready,
    Committed,
    Failed,
    Interrupted,
}

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct ImportJob {
    pub import_id: Uuid,
    pub state: ImportState,
    #[specta(type = specta_typescript::Number)]
    pub uploaded_bytes: i64,
    #[specta(type = specta_typescript::Number)]
    pub verified_images: i64,
    pub failure_code: Option<ImportFailure>,
}

#[derive(serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportFailure {
    Limit,
    Input,
    Storage,
    Archive,
    Signature,
    Authorization,
    Image,
    Conflict,
    Interrupted,
}

#[derive(Debug, Snafu)]
pub(crate) enum ImportError {
    #[snafu(display("Photo upload capacity is exhausted"))]
    Busy,
    #[snafu(display("Photo import does not exist"))]
    NotFound,
    #[snafu(display("Photo upload is empty or exceeds its resource limit"))]
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

impl PhotoComponent {
    pub(crate) async fn read_import(&self, id: Uuid) -> Result<ImportJob, ImportError> {
        self.database
            .read(move |tx| db::imports::read(tx, id))
            .await
            .map_err(TransactionError::into_error)
            .context(PersistenceSnafu)?
            .ok_or_else(|| NotFoundSnafu.build())
    }

    pub(crate) async fn upload_results<R>(&self, input: R) -> Result<ImportJob, ImportError>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let permit = self
            .upload_work
            .clone()
            .try_acquire_owned()
            .map_err(|_| BusySnafu.build())?;
        let id = Uuid::now_v7();
        let admitted = self
            .database
            .write(move |tx| {
                if db::imports::active_count(tx)? != 0 {
                    return Ok::<_, db::PersistenceError>(false);
                }
                db::imports::begin(tx, id)?;
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
            // The actual blocking operation owns capacity even if its caller disappears.
            let _permit = permit;
            let mut owned = false;
            let mut total = 0_u64;
            let result = (|| {
                let mut files = store.create_upload(id).context(StorageSnafu)?;
                owned = true;
                files.directory.sync_all().context(IoSnafu)?;
                let mut input =
                    tokio_util::io::SyncIoBridge::new_with_handle(input, runtime.clone());
                let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
                let mut checkpoint = 0;
                loop {
                    let read = input.read(&mut buffer).context(InputSnafu)?;
                    if read == 0 {
                        break;
                    }
                    total = total
                        .checked_add(u64::try_from(read).map_err(|_| LimitSnafu.build())?)
                        .ok_or_else(|| LimitSnafu.build())?;
                    ensure!(
                        total <= natsume_photo_protocol::results::PACKAGE_LIMIT,
                        LimitSnafu
                    );
                    files.file.write_all(&buffer[..read]).context(IoSnafu)?;
                    if total - checkpoint >= 1024 * 1024 {
                        runtime
                            .block_on(
                                database.write(move |tx| db::imports::progress(tx, id, total)),
                            )
                            .map_err(TransactionError::into_error)
                            .context(PersistenceSnafu)?;
                        checkpoint = total;
                    }
                }
                ensure!(total != 0, EmptySnafu);
                files.file.sync_all().context(IoSnafu)?;
                files.directory.sync_all().context(IoSnafu)?;
                runtime
                    .block_on(database.write(move |tx| db::imports::finish(tx, id, total, None)))
                    .map_err(TransactionError::into_error)
                    .context(PersistenceSnafu)?;
                Ok::<_, ImportError>(())
            })();
            if let Err(error) = result {
                // Only unlink a name exclusively created by this admitted operation.
                if owned && let Err(cleanup) = store.remove_upload(id) {
                    tracing::error!(%cleanup, "Photo partial upload cleanup failed");
                    // Leave the durable uploading fact to block further admission.
                    return Err(error);
                }
                let failure = match error {
                    ImportError::Limit => "limit",
                    ImportError::Input { .. } | ImportError::Empty => "input",
                    _ => "storage",
                };
                if let Err(persistence) = runtime.block_on(database.write(move |tx| {
                    db::imports::finish(
                        tx,
                        id,
                        total.min(natsume_photo_protocol::results::PACKAGE_LIMIT),
                        Some(failure),
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
