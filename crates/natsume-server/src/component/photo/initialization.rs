use std::collections::BTreeSet;

use snafu::{IntoError as _, OptionExt as _, ResultExt as _, Snafu, ensure};
use tokio::sync::OwnedSemaphorePermit;
use uuid::Uuid;

use super::{
    CredentialUse, PhotoComponent, db,
    enrollment::{self, Credential, EnrollmentError},
};
use crate::db::{PersistenceError, TransactionError};

use natsume_photo_protocol::{IMAGE_LIMIT, MANIFEST_LIMIT as SNAPSHOT_LIMIT, RESOURCE_LIMIT};

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub(crate) enum InitializationError {
    #[snafu(display("Photo storage task failed"))]
    Worker { source: tokio::task::JoinError },
    #[snafu(context(false), display("Photo initialization authentication failed"))]
    Enrollment { source: EnrollmentError },
    #[snafu(display("Photo enrollment has not been confirmed"))]
    Unconfirmed,
    #[snafu(display("Photo initialization request is invalid"))]
    InvalidRequest,
    #[snafu(display("Photo initialization conflicts with the recorded snapshot"))]
    Conflict,
    #[snafu(display("Photo initialization or resource does not exist"))]
    NotFound,
    #[snafu(display("Photo initialization capacity is exhausted; retry later"))]
    Busy,
    #[snafu(display("Photo initialization exceeds its resource limits"))]
    Limit,
    #[snafu(display("Import a nonempty team roster before Photo initialization"))]
    EmptyRoster,
    #[snafu(display("Photo initialization cache is inconsistent"))]
    Corrupt,
    #[snafu(display("Photo initialization persistence failed"))]
    Persistence { source: PersistenceError },
    #[snafu(display("Photo initialization storage operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("Photo initialization encoding failed"))]
    Json { source: serde_json::Error },
}
impl From<PersistenceError> for InitializationError {
    fn from(source: PersistenceError) -> Self {
        PersistenceSnafu.into_error(source)
    }
}

pub(crate) use natsume_photo_protocol::{PhotoInitialization, PhotoLogo};
pub(crate) struct SnapshotDownload {
    pub json: String,
    pub sha256: String,
}
pub(crate) struct ResourceDownload {
    pub bytes: Vec<u8>,
    pub permit: OwnedSemaphorePermit,
}

fn snapshot(record: db::InitializationRecord) -> Result<SnapshotDownload, InitializationError> {
    ensure!(
        record.json.len() <= SNAPSHOT_LIMIT
            && hex::encode(enrollment::hash(record.json.as_bytes())) == record.hash,
        CorruptSnafu
    );
    Ok(SnapshotDownload {
        json: record.json,
        sha256: record.hash,
    })
}

pub(crate) enum Preparation {
    Existing(SnapshotDownload),
    Build(Box<InitializationDraft>),
}
pub(crate) struct InitializationDraft {
    credential: Credential,
    permit: OwnedSemaphorePermit,
    files: super::storage::SnapshotFiles,
    stored: BTreeSet<String>,
    total: u64,
    pub manifest: PhotoInitialization,
}
impl InitializationDraft {
    pub(crate) async fn add_logo(
        &mut self,
        organization_id: i64,
        bytes: Vec<u8>,
    ) -> Result<(), InitializationError> {
        let size_bytes = bytes.len() as u64;
        ensure!(size_bytes <= IMAGE_LIMIT as u64, LimitSnafu);
        let sha256 = hex::encode(enrollment::hash(&bytes));
        if self.stored.insert(sha256.clone()) {
            self.total += size_bytes;
            ensure!(self.total <= RESOURCE_LIMIT, LimitSnafu);
            self.files.put(sha256.clone(), bytes).await?;
        }
        self.manifest.school_logos.push(PhotoLogo {
            organization_id,
            sha256,
            content_type: "image/png".into(),
            size_bytes,
        });
        Ok(())
    }
}

impl PhotoComponent {
    /// Immutable once published. Files are fully synced before the database
    /// references them. Interrupted unpublished directories are retained, not reused.
    pub(crate) async fn begin_initialization(
        &self,
        token: &str,
    ) -> Result<Preparation, InitializationError> {
        let credential = enrollment::credential(token)?;
        let current = self.read_snapshot(&credential).await?;
        if let Some(record) = current {
            return snapshot(record).map(Preparation::Existing);
        }
        let permit = self
            .snapshot_work
            .clone()
            .try_acquire_owned()
            .map_err(|_| BusySnafu.build())?;
        // Another request may have published while we waited for admission.
        if let Some(record) = self.read_snapshot(&credential).await? {
            return snapshot(record).map(Preparation::Existing);
        }
        let identity = credential.clone();
        let dataset_id = self
            .database
            .write(move |tx| -> Result<_, InitializationError> {
                enrollment::authorized(tx, &identity, CredentialUse::Initialization)?;
                Ok(db::dataset_id(tx, &Uuid::now_v7().to_string())?)
            })
            .await
            .map_err(TransactionError::into_error)?;
        let id = Uuid::now_v7().to_string();
        let files = self.storage.begin(id.clone()).await?;
        let source_id = credential.source.clone();
        Ok(Preparation::Build(Box::new(InitializationDraft {
            credential,
            permit,
            files,
            stored: BTreeSet::new(),
            total: 0,
            manifest: PhotoInitialization {
                format_version: 1,
                source_id,
                dataset_id,
                initialization_id: id,
                teams: Vec::new(),
                school_logos: Vec::new(),
            },
        })))
    }

    pub(crate) async fn publish_initialization(
        &self,
        draft: InitializationDraft,
    ) -> Result<SnapshotDownload, InitializationError> {
        let InitializationDraft {
            credential,
            permit: _permit,
            files,
            manifest,
            ..
        } = draft;
        manifest
            .validate(&manifest.source_id)
            .map_err(|_| CorruptSnafu.build())?;
        let json = serde_json::to_string(&manifest).context(JsonSnafu)?;
        ensure!(json.len() <= SNAPSHOT_LIMIT, LimitSnafu);
        files.finish().await?;
        let hash = hex::encode(enrollment::hash(json.as_bytes()));
        let id = manifest.initialization_id;
        self.database
            .write(move |tx| -> Result<_, InitializationError> {
                let source =
                    enrollment::authorized(tx, &credential, CredentialUse::Initialization)?;
                ensure!(source.confirmed, UnconfirmedSnafu);
                if let Some(existing) = db::initialization(tx, &source.id)? {
                    return snapshot(existing);
                }
                db::insert_initialization(tx, &source.id, &id, &json, &hash)?;
                Ok(SnapshotDownload { json, sha256: hash })
            })
            .await
            .map_err(TransactionError::into_error)
    }

    async fn read_snapshot(
        &self,
        credential: &Credential,
    ) -> Result<Option<db::InitializationRecord>, InitializationError> {
        let credential = credential.clone();
        self.database
            .read(move |tx| -> Result<_, InitializationError> {
                let source =
                    enrollment::authorized(tx, &credential, CredentialUse::Initialization)?;
                ensure!(source.confirmed, UnconfirmedSnafu);
                Ok(db::initialization(tx, &source.id)?)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn resource(
        &self,
        token: &str,
        hash: &str,
    ) -> Result<ResourceDownload, InitializationError> {
        let credential = enrollment::credential(token)?;
        let record = self
            .read_snapshot(&credential)
            .await?
            .context(NotFoundSnafu)?;
        let download = snapshot(record)?;
        let manifest: PhotoInitialization =
            serde_json::from_str(&download.json).context(JsonSnafu)?;
        ensure!(
            manifest.source_id == credential.source
                && enrollment::canonical_id(&manifest.initialization_id).is_ok(),
            CorruptSnafu
        );
        let logo = manifest
            .school_logos
            .iter()
            .find(|logo| logo.sha256 == hash)
            .context(NotFoundSnafu)?;
        ensure!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            CorruptSnafu
        );
        let permit = self
            .download_work
            .clone()
            .try_acquire_owned()
            .map_err(|_| BusySnafu.build())?;
        let bytes = self
            .storage
            .read(manifest.initialization_id, hash.to_owned(), logo.size_bytes)
            .await?;
        Ok(ResourceDownload { bytes, permit })
    }

    pub(crate) async fn complete(
        &self,
        token: &str,
        id: String,
        hash: String,
        request: String,
    ) -> Result<(), InitializationError> {
        let credential = enrollment::credential(token)?;
        ensure!(
            enrollment::canonical_id(&id).is_ok() && enrollment::canonical_id(&request).is_ok(),
            InvalidRequestSnafu
        );
        self.database
            .write(move |tx| -> Result<_, InitializationError> {
                let source = enrollment::authorized(tx, &credential, CredentialUse::Completion)?;
                ensure!(source.confirmed, UnconfirmedSnafu);
                let record = db::initialization(tx, &source.id)?.context(NotFoundSnafu)?;
                ensure!(record.id == id && record.hash == hash, ConflictSnafu);
                if let Some(previous) = record.completion_request {
                    ensure!(
                        previous == request && record.completed_at.is_some(),
                        ConflictSnafu
                    );
                    return Ok(());
                }
                let now = db::now(tx)?;
                db::complete(tx, &id, &request, now)?;
                Ok(())
            })
            .await
            .map_err(TransactionError::into_error)
    }
}
