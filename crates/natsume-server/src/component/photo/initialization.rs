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
    #[snafu(display("Photo roster contains invalid Server facts"))]
    InvalidRoster {
        source: natsume_photo_protocol::ValidationError,
    },
    #[snafu(display("Photo snapshot contract is inconsistent"))]
    InvalidContract {
        source: natsume_photo_protocol::ValidationError,
    },
    #[snafu(display("Photo persisted snapshot identity is invalid"))]
    InvalidSnapshot {
        source: natsume_photo_protocol::ValueError,
    },
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

use natsume_photo_protocol::{
    DatasetId, FormatVersion, InitializationId, LogoContentType, PhotoInitialization,
    PhotoInitializationComplete, PhotoLogo, PhotoTeam, Sha256Digest, SourceId,
};
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
    stored: BTreeSet<Sha256Digest>,
    total: u64,
    manifest: PhotoInitialization,
}
impl InitializationDraft {
    pub(crate) fn set_teams(&mut self, teams: Vec<PhotoTeam>) -> Result<(), InitializationError> {
        ensure!(
            self.manifest.teams.is_empty() && self.manifest.school_logos.is_empty(),
            ConflictSnafu
        );
        ensure!(!teams.is_empty(), EmptyRosterSnafu);
        natsume_photo_protocol::validate_teams(&teams).context(InvalidRosterSnafu)?;
        self.manifest.teams = teams;
        Ok(())
    }

    pub(crate) async fn add_logo(
        &mut self,
        organization_id: i64,
        bytes: Vec<u8>,
    ) -> Result<(), InitializationError> {
        let size_bytes = bytes.len() as u64;
        ensure!((1..=IMAGE_LIMIT as u64).contains(&size_bytes), LimitSnafu);
        if !self
            .manifest
            .teams
            .iter()
            .any(|team| team.organization_id == organization_id)
            || self
                .manifest
                .school_logos
                .iter()
                .any(|logo| logo.organization_id == organization_id)
        {
            return Err(InvalidContractSnafu
                .into_error(natsume_photo_protocol::ValidationError::SchoolLogo));
        }
        let sha256 = Sha256Digest::of(&bytes);
        if !self.stored.contains(&sha256) {
            ensure!(
                size_bytes <= RESOURCE_LIMIT.saturating_sub(self.total),
                LimitSnafu
            );
            self.files.put(sha256.clone(), bytes).await?;
            self.stored.insert(sha256.clone());
            self.total += size_bytes;
        }
        self.manifest.school_logos.push(PhotoLogo {
            organization_id,
            sha256,
            content_type: LogoContentType::Png,
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
        let id = InitializationId::generate();
        let files = self.storage.begin(id.clone()).await?;
        let source_id: SourceId = credential.source.parse().context(InvalidSnapshotSnafu)?;
        let dataset_id: DatasetId = dataset_id.parse().context(InvalidSnapshotSnafu)?;
        Ok(Preparation::Build(Box::new(InitializationDraft {
            credential,
            permit,
            files,
            stored: BTreeSet::new(),
            total: 0,
            manifest: PhotoInitialization {
                format_version: FormatVersion::V1,
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
            .context(InvalidContractSnafu)?;
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
                db::insert_initialization(tx, &source.id, id.as_str(), &json, &hash)?;
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
            manifest.source_id.as_str() == credential.source,
            CorruptSnafu
        );
        let logo = manifest
            .school_logos
            .iter()
            .find(|logo| logo.sha256.as_str() == hash)
            .context(NotFoundSnafu)?;
        let permit = self
            .download_work
            .clone()
            .try_acquire_owned()
            .map_err(|_| BusySnafu.build())?;
        let bytes = self
            .storage
            .read(
                manifest.initialization_id,
                logo.sha256.clone(),
                logo.size_bytes,
            )
            .await?;
        Ok(ResourceDownload { bytes, permit })
    }

    pub(crate) async fn complete(
        &self,
        token: &str,
        receipt: PhotoInitializationComplete,
    ) -> Result<(), InitializationError> {
        let credential = enrollment::credential(token)?;
        let PhotoInitializationComplete {
            initialization_id: id,
            manifest_sha256: hash,
            request_id: request,
        } = receipt;
        self.database
            .write(move |tx| -> Result<_, InitializationError> {
                let source = enrollment::authorized(tx, &credential, CredentialUse::Completion)?;
                ensure!(source.confirmed, UnconfirmedSnafu);
                let record = db::initialization(tx, &source.id)?.context(NotFoundSnafu)?;
                ensure!(
                    record.id == id.as_str() && record.hash == hash.as_str(),
                    ConflictSnafu
                );
                if let Some(previous) = record.completion_request {
                    ensure!(
                        previous == request.as_str() && record.completed_at.is_some(),
                        ConflictSnafu
                    );
                    return Ok(());
                }
                let now = db::now(tx)?;
                db::complete(tx, id.as_str(), request.as_str(), now)?;
                Ok(())
            })
            .await
            .map_err(TransactionError::into_error)
    }
}
