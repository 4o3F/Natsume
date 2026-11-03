use super::{
    AuthorizationSnafu, ConflictSnafu, CorruptSnafu, InterruptedSnafu, JsonSnafu, PersistenceSnafu,
    Result, SignatureSnafu, SnapshotContractSnafu, SnapshotJsonSnafu, StorageSnafu,
    ValidationError, WorkerSnafu, archive::Package,
};
use crate::{
    component::photo::{PhotoComponent, SourceState, db},
    db::TransactionError,
};
use natsume_photo_protocol::{
    PhotoInitialization, Sha256Digest,
    results::{ResultContext, SignedResults},
};
use snafu::{OptionExt as _, ResultExt as _, ensure};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

impl PhotoComponent {
    pub(crate) async fn maintain_results(
        self: Arc<Self>,
        stop: CancellationToken,
        ready: oneshot::Sender<()>,
    ) -> Result<()> {
        let store = self.results.clone();
        let lease = tokio::task::spawn_blocking(move || store.worker_lease())
            .await
            .context(WorkerSnafu)?
            .context(StorageSnafu)?;
        let interrupted = self
            .database
            .read(db::validation::uploading)
            .await
            .map_err(TransactionError::into_error)
            .context(PersistenceSnafu)?;
        ensure!(interrupted.len() <= 1, CorruptSnafu);
        for id in interrupted {
            let this = self.clone();
            let ownership = lease.clone();
            tokio::task::spawn_blocking(move || {
                let _lease = ownership;
                this.discard_upload(id)?;
                tokio::runtime::Handle::current()
                    .block_on(
                        this.database
                            .write(move |tx| db::validation::fail(tx, id, true, "interrupted")),
                    )
                    .map_err(TransactionError::into_error)
                    .context(PersistenceSnafu)
            })
            .await
            .context(WorkerSnafu)??;
        }
        let _ready = ready.send(());
        loop {
            if stop.is_cancelled() {
                return Ok(());
            }
            let pending = self
                .database
                .read(db::validation::pending)
                .await
                .map_err(TransactionError::into_error)
                .context(PersistenceSnafu)?;
            if let Some((id, bytes)) = pending {
                self.database
                    .write(move |tx| db::validation::claim(tx, id))
                    .await
                    .map_err(TransactionError::into_error)
                    .context(PersistenceSnafu)?;
                let this = self.clone();
                let owned_stop = stop.clone();
                let ownership = lease.clone();
                tokio::task::spawn_blocking(move || {
                    let _lease = ownership;
                    this.process(id, bytes, &owned_stop)
                })
                .await
                .context(WorkerSnafu)??;
                continue;
            }
            tokio::select! {
                () = stop.cancelled() => return Ok(()),
                () = self.validation_wake.notified() => {}
            }
        }
    }

    fn discard_upload(&self, id: Uuid) -> Result<()> {
        match self.results.remove_upload(id) {
            Ok(()) => Ok(()),
            Err(super::super::results_storage::ResultsStorageError::Io { source })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(())
            }
            Err(source) => Err(ValidationError::Storage { source }),
        }
    }

    fn process(&self, id: Uuid, bytes: u64, stop: &CancellationToken) -> Result<()> {
        let result = self.validate(id, bytes, stop);
        if matches!(result, Err(ValidationError::Interrupted)) {
            return Ok(());
        }
        if let Err(error) = result {
            let code = match &error {
                ValidationError::Archive
                | ValidationError::Zip { .. }
                | ValidationError::Json { .. } => "archive",
                ValidationError::Io { source }
                    if matches!(
                        source.kind(),
                        std::io::ErrorKind::UnexpectedEof
                            | std::io::ErrorKind::InvalidData
                            | std::io::ErrorKind::InvalidInput
                    ) =>
                {
                    "archive"
                }
                ValidationError::Signature { .. } => "signature",
                ValidationError::Storage {
                    source: super::super::results_storage::ResultsStorageError::Io { source },
                } if source.kind() == std::io::ErrorKind::NotFound => "storage",
                ValidationError::Authorization => "authorization",
                ValidationError::Integrity
                | ValidationError::Image { .. }
                | ValidationError::Allocation { .. } => "image",
                ValidationError::Conflict => "conflict",
                _ => return Err(error),
            };
            tracing::warn!(%id, %error, "Photo result package rejected");
            self.discard_upload(id)?;
            tokio::runtime::Handle::current()
                .block_on(
                    self.database
                        .write(move |tx| db::validation::fail(tx, id, false, code)),
                )
                .map_err(TransactionError::into_error)
                .context(PersistenceSnafu)?;
        } else {
            self.discard_upload(id)?;
        }
        Ok(())
    }

    fn validate(&self, id: Uuid, bytes: u64, stop: &CancellationToken) -> Result<()> {
        ensure!(!stop.is_cancelled(), InterruptedSnafu);
        let file = self.results.open_upload(id).context(StorageSnafu)?;
        let mut package = Package::open(file, bytes)?;
        let signed = package.signed.clone();
        let runtime = tokio::runtime::Handle::current();
        let authorization = signed.clone();
        runtime
            .block_on(self.database.read(move |tx| trusted(tx, &authorization)))
            .map_err(TransactionError::into_error)?;
        let mut stage = self.results.stage().context(StorageSnafu)?;
        let verification = package.images(&mut stage, stop, |count| {
            runtime
                .block_on(
                    self.database
                        .write(move |tx| db::validation::progress(tx, id, count)),
                )
                .map_err(TransactionError::into_error)
                .context(PersistenceSnafu)
        });
        let images = match verification {
            Ok(images) => images,
            Err(error) => {
                stage.cleanup().context(StorageSnafu)?;
                return Err(error);
            }
        };
        let publication = (|| {
            for image in &images {
                ensure!(!stop.is_cancelled(), InterruptedSnafu);
                self.results.publish(&stage, image).context(StorageSnafu)?;
            }
            self.results.sync_objects().context(StorageSnafu)
        })();
        stage.cleanup().context(StorageSnafu)?;
        publication?;
        ensure!(!stop.is_cancelled(), InterruptedSnafu);
        let hash = Sha256Digest::of(&signed.manifest.signing_bytes().context(SignatureSnafu)?);
        let json = serde_json::to_string(signed.as_ref()).context(JsonSnafu)?;
        ensure!(
            json.len() <= natsume_photo_protocol::MANIFEST_LIMIT,
            super::ArchiveSnafu
        );
        runtime
            .block_on(self.database.write(move |tx| {
                trusted(tx, &signed)?;
                ensure!(
                    !db::validation::manifest_conflicts(tx, &signed, &hash)
                        .context(PersistenceSnafu)?,
                    ConflictSnafu
                );
                for image in &images {
                    db::validation::object(tx, image).context(PersistenceSnafu)?;
                }
                db::validation::ready(tx, id, &signed, &json, &hash).context(PersistenceSnafu)
            }))
            .map_err(TransactionError::into_error)
    }
}

pub(in crate::component::photo) fn trusted(
    tx: &mut crate::db::Transaction<'_>,
    signed: &SignedResults,
) -> Result<()> {
    let context = &signed.manifest.context;
    let source = db::source(tx, &context.source_id)
        .context(PersistenceSnafu)?
        .context(AuthorizationSnafu)?;
    ensure!(
        source.state == SourceState::Enabled && source.confirmed,
        AuthorizationSnafu
    );
    let key = source.public_key.context(AuthorizationSnafu)?;
    let snapshot = db::initialization(tx, &context.source_id)
        .context(PersistenceSnafu)?
        .context(AuthorizationSnafu)?;
    ensure!(
        snapshot.completed_at.is_some()
            && snapshot.id == context.initialization_id
            && snapshot.hash == context.initialization_sha256,
        AuthorizationSnafu
    );
    ensure!(
        Sha256Digest::of(snapshot.json.as_bytes()) == snapshot.hash,
        CorruptSnafu
    );
    let initialization: PhotoInitialization =
        serde_json::from_str(&snapshot.json).context(SnapshotJsonSnafu)?;
    initialization
        .validate(&context.source_id)
        .context(SnapshotContractSnafu)?;
    let expected = ResultContext {
        source_id: initialization.source_id,
        dataset_id: initialization.dataset_id,
        initialization_id: snapshot.id,
        initialization_sha256: snapshot.hash,
    };
    signed.verify(&expected, &key).context(SignatureSnafu)?;
    let teams: BTreeMap<_, _> = initialization
        .teams
        .into_iter()
        .map(|team| (team.account_id, team.account))
        .collect();
    ensure!(
        signed
            .manifest
            .results
            .iter()
            .all(|result| teams.get(&result.account_id) == Some(&result.account)),
        AuthorizationSnafu
    );
    Ok(())
}
