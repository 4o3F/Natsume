//! Workspace business rules, separate from storage assembly and typed queries.
use crate::roster::Roster;
use db::Database;
use natsume_photo_protocol::PhotoTeam;
use resources::{ImageRef, Prepared};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

mod db;
mod final_photos;
pub(crate) mod resources;
mod storage;
pub use db::PersistenceError;
pub use final_photos::{ConfirmedPhoto, FinalPhotoError};
pub(crate) use final_photos::{FinalDraft, FinalPhotos};
pub(crate) use final_photos::{FinalSnapshot, SnapshotImage};
pub use resources::ResourceError;
pub use storage::StorageError;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("confirmed photo validation failed: {source}"))]
    FinalPhoto { source: FinalPhotoError },
    #[snafu(display("workspace storage failed: {source}"))]
    Storage { source: StorageError },
    #[snafu(display("workspace persistence failed: {source}"))]
    Persistence { source: PersistenceError },
    #[snafu(display("workspace preparation is incomplete"))]
    Incomplete,
    #[snafu(display("workspace readiness marker is corrupt"))]
    InvalidReadiness,
    #[snafu(display("workspace roster is invalid: {source}"))]
    Roster { source: crate::roster::Error },
    #[snafu(display("workspace school logo is invalid: {source}"))]
    Resource { source: ResourceError },
    #[snafu(display("team is not in this workspace"))]
    UnknownTeam,
    #[snafu(display("workspace resource task failed"))]
    Worker { source: tokio::task::JoinError },
}
pub type Result<T> = std::result::Result<T, Error>;
/// Exclusive filesystem ownership permitting capture and workspace writes.
pub struct Writable(WorkspaceWriter);
/// Shared ownership of an acquired workspace write lock; cannot be constructed by callers.
#[derive(Clone)]
pub(crate) struct WorkspaceWriter {
    _handle: std::sync::Arc<std::fs::File>,
}
/// Read-only access; cannot be passed to capture or mutation APIs.
pub struct ReadOnly;
/// A fully validated workspace with access selected by its type.
pub struct Workspace<Access = Writable> {
    root: PathBuf,
    database: Database,
    access: Access,
    final_operations: std::sync::Arc<std::sync::Mutex<()>>,
}

impl Workspace {
    // Owned background export state shares the same write capability and file-operation fence.
    pub(crate) fn share_for_export(&self) -> Self {
        Self {
            root: self.root.clone(),
            database: self.database.clone(),
            access: Writable(self.writable_owner()),
            final_operations: self.final_operations.clone(),
        }
    }
    pub(crate) fn create_with_logos(
        root: PathBuf,
        teams: Vec<PhotoTeam>,
        source: &Path,
        logos: Vec<resources::SchoolLogo>,
    ) -> Result<Self> {
        let organizations = teams.iter().map(|team| team.organization_id).collect();
        let prepared =
            Prepared::from_paths(source, logos, &organizations).context(ResourceSnafu)?;
        Self::create_blocking(root, teams, prepared)
    }
    fn create_blocking(root: PathBuf, teams: Vec<PhotoTeam>, prepared: Prepared) -> Result<Self> {
        let (root, database, writer) = storage::create(root).context(StorageSnafu)?;
        let cache = prepared.copy_to(&root).context(ResourceSnafu)?;
        database
            .save_roster_blocking(teams, cache)
            .context(PersistenceSnafu)?;
        Ok(Self {
            root,
            database,
            access: Writable(writer),
            final_operations: std::sync::Arc::default(),
        })
    }
    pub(crate) fn publish_verified(verified: WorkspaceReader, target: &Path) -> Result<Self> {
        let Workspace { root, database, .. } = verified;
        // Windows directory publication requires closing SQLite before rename.
        drop(database);
        let (root, database, writer) =
            storage::publish_verified(root, target).context(StorageSnafu)?;
        Ok(Self {
            root,
            database,
            access: Writable(writer),
            final_operations: std::sync::Arc::default(),
        })
    }
    /// Opens and validates a workspace while retaining exclusive write ownership.
    /// # Errors
    /// Rejects concurrent writers, invalid metadata, roster or resources.
    pub async fn open(root: &Path) -> Result<Self> {
        let (root, database, owner) = storage::open_writer(root.to_owned())
            .await
            .context(StorageSnafu)?;
        let workspace = Self {
            root,
            database,
            access: Writable(owner),
            final_operations: std::sync::Arc::default(),
        };
        workspace.verify().await?;
        Ok(workspace)
    }
    pub(crate) fn writable_owner(&self) -> WorkspaceWriter {
        self.access.0.clone()
    }
    pub(crate) fn final_store(&self) -> FinalPhotos {
        FinalPhotos::new(
            self.root.clone(),
            self.database.clone(),
            self.final_operations.clone(),
        )
    }
    pub(crate) async fn export_snapshot(&self) -> Result<FinalSnapshot> {
        let accounts = self
            .roster()
            .await?
            .teams()
            .iter()
            .map(|team| (team.account_id.clone(), team.account.clone()))
            .collect();
        let store = self.final_store();
        tokio::task::spawn_blocking(move || store.snapshot(&accounts))
            .await
            .context(WorkerSnafu)?
            .context(FinalPhotoSnafu)
    }
}
/// Read-only view of a fully validated workspace.
/// Read access cannot authorize capture:
/// ```compile_fail
/// use natsume_photo_client::{capture_session::CaptureSession, workspace::WorkspaceReader};
/// async fn capture(reader: &WorkspaceReader, folder: &std::path::Path) {
///     CaptureSession::prepare(reader, folder).await;
/// }
/// ```
pub type WorkspaceReader = Workspace<ReadOnly>;
impl Workspace<ReadOnly> {
    /// Opens without creating locks, installing migrations or writing files.
    /// # Errors
    /// Rejects invalid metadata, roster or resources.
    pub async fn inspect(root: &Path) -> Result<Self> {
        let (root, database) = storage::open_reader(root.to_owned())
            .await
            .context(StorageSnafu)?;
        let workspace = Self {
            root,
            database,
            access: ReadOnly,
            final_operations: std::sync::Arc::default(),
        };
        workspace.verify().await?;
        Ok(workspace)
    }
}
impl<Access> Workspace<Access> {
    /// Maps verified current results to the shared contract using the immutable roster.
    /// This reads metadata only; it does not pin files or create an export snapshot.
    /// # Errors
    /// Reports invalid current files, persistence failures or missing roster identities.
    pub async fn current_result_entries(
        &self,
    ) -> Result<Vec<natsume_photo_protocol::results::TeamResult>> {
        let roster = self.roster().await?;
        let accounts: std::collections::BTreeMap<_, _> = roster
            .teams()
            .iter()
            .map(|team| (team.account_id.as_str(), team.account.as_str()))
            .collect();
        self.confirmed_photos()
            .await?
            .into_iter()
            .map(|photo| {
                let account = accounts
                    .get(photo.account_id())
                    .ok_or_else(|| UnknownTeamSnafu.build())?;
                photo
                    .into_result((*account).to_owned())
                    .context(FinalPhotoSnafu)
            })
            .collect()
    }
    /// Reads and verifies current confirmed files without restoring unfinished captures.
    /// # Errors
    /// Reports corrupt references, missing files, hash mismatches and persistence failures.
    pub async fn confirmed_photos(&self) -> Result<Vec<ConfirmedPhoto>> {
        let store = FinalPhotos::new(
            self.root.clone(),
            self.database.clone(),
            self.final_operations.clone(),
        );
        tokio::task::spawn_blocking(move || store.list())
            .await
            .context(WorkerSnafu)?
            .context(FinalPhotoSnafu)
    }
    async fn verify(&self) -> Result<()> {
        let metadata = self.database.metadata().await.context(PersistenceSnafu)?;
        ensure!(matches!(metadata.ready, 0 | 1), InvalidReadinessSnafu);
        ensure!(metadata.ready == 1, IncompleteSnafu);
        self.roster().await?;
        self.verify_resources().await?;
        self.confirmed_photos().await?;
        Ok(())
    }
    pub(crate) async fn verify_resources(&self) -> Result<()> {
        let rows = self.database.logos().await.context(PersistenceSnafu)?;
        let references: BTreeSet<_> = rows
            .into_iter()
            .map(|(path, hash)| ImageRef::from_columns(path, hash))
            .collect::<std::result::Result<Vec<_>, _>>()
            .context(ResourceSnafu)?
            .into_iter()
            .flatten()
            .collect();
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            for reference in references {
                reference.verify(&root)?;
            }
            Ok(())
        })
        .await
        .context(WorkerSnafu)?
        .context(ResourceSnafu)?;
        Ok(())
    }
    /// Reads a validated roster sorted by external Team ID.
    /// # Errors
    /// Reports persistence failures and invalid persisted identities.
    pub async fn roster(&self) -> Result<Roster> {
        Roster::from_teams(self.database.teams().await.context(PersistenceSnafu)?)
            .context(RosterSnafu)
    }
    /// Reads a known team's verified school logo; absence remains empty.
    /// # Errors
    /// Rejects unknown teams and damaged or missing configured logos.
    pub async fn logo_path(&self, account_id: &str) -> Result<Option<PathBuf>> {
        let (path, hash) = self
            .database
            .logo(account_id.to_owned())
            .await
            .context(PersistenceSnafu)?
            .ok_or_else(|| UnknownTeamSnafu.build())?;
        let reference = ImageRef::from_columns(path, hash).context(ResourceSnafu)?;
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || reference.map(|r| r.verify(&root)).transpose())
            .await
            .context(WorkerSnafu)?
            .context(ResourceSnafu)
    }
    pub(crate) async fn matches_logos(
        &self,
        manifest: &natsume_photo_protocol::PhotoInitialization,
    ) -> Result<bool> {
        let teams = self.roster().await?;
        let expected: std::collections::BTreeMap<_, _> = manifest
            .school_logos
            .iter()
            .map(|logo| (logo.organization_id, logo.sha256.as_str()))
            .collect();
        let entries: std::collections::BTreeMap<_, _> = self
            .database
            .logo_entries()
            .await
            .context(PersistenceSnafu)?
            .into_iter()
            .map(|(account, path, hash)| (account, (path, hash)))
            .collect();
        for team in teams.teams() {
            let columns = entries.get(&team.account_id).cloned();
            let wanted = expected.get(&team.organization_id);
            match (columns, wanted) {
                (Some((None, None)), None) => {}
                (Some((Some(path), Some(hash))), Some(wanted))
                    if hash == *wanted && path == format!("resources/{hash}.png") => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
#[path = "../../tests/unit/workspace_publication.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/unit/workspace_operations.rs"]
mod workspace_operations;

#[cfg(test)]
#[path = "../../tests/unit/resource_operations.rs"]
mod resource_operations;

#[cfg(test)]
#[path = "../../tests/unit/workspace_fixture.rs"]
pub(crate) mod fixture;
