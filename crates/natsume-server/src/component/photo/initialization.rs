use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use snafu::{IntoError as _, OptionExt as _, ResultExt as _, Snafu, ensure};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    sync::OwnedSemaphorePermit,
};
use uuid::Uuid;

use super::{
    CredentialUse, PhotoComponent, db,
    enrollment::{self, Credential, EnrollmentError},
};
use crate::{
    component::contest::{ContestComponent, ContestError, ExportError, PhotoTeam},
    db::{PersistenceError, TransactionError},
};

const IMAGE_LIMIT: u64 = 16 * 1024 * 1024;
const SNAPSHOT_LIMIT: usize = 4 * 1024 * 1024;
const RESOURCE_LIMIT: u64 = 256 * 1024 * 1024;

#[derive(Debug, Snafu)]
pub(crate) enum InitializationError {
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
    #[snafu(display("Photo roster could not be read"))]
    Roster { source: ContestError },
    #[snafu(display("Photo school logo preparation failed"))]
    Logo { source: ExportError },
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

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhotoLogo {
    #[specta(type = specta_typescript::Number)]
    pub organization_id: i64,
    pub sha256: String,
    pub content_type: String,
    #[specta(type = specta_typescript::Number)]
    pub size_bytes: u64,
}
#[derive(Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhotoInitialization {
    pub format_version: u32,
    pub source_id: String,
    pub dataset_id: String,
    pub initialization_id: String,
    pub teams: Vec<PhotoTeam>,
    pub school_logos: Vec<PhotoLogo>,
}
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

async fn root_directory(root: &Path) -> Result<PathBuf, InitializationError> {
    ensure!(root.is_absolute(), CorruptSnafu);
    tokio::fs::create_dir_all(root).await.context(IoSnafu)?;
    tokio::fs::canonicalize(root).await.context(IoSnafu)
}

impl PhotoComponent {
    /// Immutable once published. Files are fully synced before the database
    /// references them. Interrupted unpublished directories are retained, not reused.
    pub(crate) async fn initialization(
        &self,
        token: &str,
        contest: &ContestComponent,
    ) -> Result<SnapshotDownload, InitializationError> {
        let credential = enrollment::credential(token)?;
        let current = self.read_snapshot(&credential).await?;
        if let Some(record) = current {
            return snapshot(record);
        }
        let _permit = self
            .snapshot_work
            .clone()
            .try_acquire_owned()
            .map_err(|_| BusySnafu.build())?;
        // Another request may have published while we waited for admission.
        if let Some(record) = self.read_snapshot(&credential).await? {
            return snapshot(record);
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
        let teams = contest.photo_roster().await.context(RosterSnafu)?;
        ensure!(!teams.is_empty(), EmptyRosterSnafu);
        ensure!(teams.len() <= 2000, LimitSnafu);
        let id = Uuid::now_v7().to_string();
        let root = root_directory(&self.directory).await?;
        let directory = root.join(&id);
        tokio::fs::create_dir(&directory).await.context(IoSnafu)?;
        let schools: BTreeMap<_, _> = teams
            .iter()
            .map(|team| (team.organization_id, team))
            .collect();
        let mut school_logos = Vec::new();
        let mut stored = BTreeSet::new();
        let mut total = 0_u64;
        for (organization_id, team) in schools {
            if let Some(bytes) = contest.photo_logo(team).await.context(LogoSnafu)? {
                let size_bytes = bytes.len() as u64;
                ensure!(size_bytes <= IMAGE_LIMIT, LimitSnafu);
                let sha256 = hex::encode(enrollment::hash(&bytes));
                if stored.insert(sha256.clone()) {
                    total += size_bytes;
                    ensure!(total <= RESOURCE_LIMIT, LimitSnafu);
                    let mut file = tokio::fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(directory.join(format!("{sha256}.png")))
                        .await
                        .context(IoSnafu)?;
                    file.write_all(&bytes).await.context(IoSnafu)?;
                    file.sync_all().await.context(IoSnafu)?;
                }
                school_logos.push(PhotoLogo {
                    organization_id,
                    sha256,
                    content_type: "image/png".into(),
                    size_bytes,
                });
            }
        }
        let manifest = PhotoInitialization {
            format_version: 1,
            source_id: credential.source.clone(),
            dataset_id,
            initialization_id: id.clone(),
            teams,
            school_logos,
        };
        let json = serde_json::to_string(&manifest).context(JsonSnafu)?;
        ensure!(json.len() <= SNAPSHOT_LIMIT, LimitSnafu);
        // Sync the directory entry publication as well as each file's contents.
        let directory_handle = tokio::fs::File::open(&directory).await.context(IoSnafu)?;
        directory_handle.sync_all().await.context(IoSnafu)?;
        let root_handle = tokio::fs::File::open(root).await.context(IoSnafu)?;
        root_handle.sync_all().await.context(IoSnafu)?;
        let hash = hex::encode(enrollment::hash(json.as_bytes()));
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
        let root = tokio::fs::canonicalize(&self.directory)
            .await
            .context(IoSnafu)?;
        let path = root
            .join(manifest.initialization_id)
            .join(format!("{hash}.png"));
        let resolved = tokio::fs::canonicalize(&path).await.context(IoSnafu)?;
        ensure!(
            resolved == path
                && !tokio::fs::symlink_metadata(&path)
                    .await
                    .context(IoSnafu)?
                    .is_symlink(),
            CorruptSnafu
        );
        let mut bytes = Vec::new();
        tokio::fs::File::open(path)
            .await
            .context(IoSnafu)?
            .take(IMAGE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .await
            .context(IoSnafu)?;
        ensure!(
            bytes.len() as u64 <= IMAGE_LIMIT
                && bytes.len() as u64 == logo.size_bytes
                && hex::encode(enrollment::hash(&bytes)) == hash,
            CorruptSnafu
        );
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
