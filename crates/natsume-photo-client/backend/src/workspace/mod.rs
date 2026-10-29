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
pub(crate) mod resources;
mod storage;
pub use db::PersistenceError;
pub use resources::ResourceError;
pub use storage::StorageError;

#[derive(Debug, Snafu)]
pub enum Error {
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
pub struct Writable(std::sync::Arc<std::fs::File>);
/// Read-only access; cannot be passed to capture or mutation APIs.
pub struct ReadOnly;
/// A fully validated workspace with access selected by its type.
pub struct Workspace<Access = Writable> {
    root: PathBuf,
    database: Database,
    access: Access,
}

impl Workspace {
    /// Creates a fresh roster workspace; production enrollment uses the initializer.
    /// # Errors
    /// Rejects existing targets or failed storage, resource or roster writes.
    pub async fn create(root: &Path, roster: &Roster) -> Result<Self> {
        Self::create_prepared(root, roster, None).await
    }
    /// Creates a new workspace with validated school logos and no fallback image.
    /// # Errors
    /// Rejects invalid logos and existing targets without overwriting them.
    pub async fn create_with_resources(
        root: &Path,
        roster: &Roster,
        manifest: &Path,
    ) -> Result<Self> {
        let manifest = manifest.to_owned();
        let organizations = roster.teams().iter().map(|t| t.organization_id).collect();
        let prepared =
            tokio::task::spawn_blocking(move || Prepared::load(&manifest, &organizations))
                .await
                .context(WorkerSnafu)?
                .context(ResourceSnafu)?;
        Self::create_prepared(root, roster, Some(prepared)).await
    }
    pub(crate) async fn create_with_logos(
        root: &Path,
        roster: &Roster,
        source: PathBuf,
        logos: Vec<resources::SchoolLogo>,
        initialization_lease: std::sync::Arc<std::fs::File>,
    ) -> Result<Self> {
        let organizations = roster
            .teams()
            .iter()
            .map(|team| team.organization_id)
            .collect();
        let teams = roster.teams().to_vec();
        let root = root.to_owned();
        tokio::task::spawn_blocking(move || {
            // This one task owns every preparation side effect, even if its caller is cancelled.
            let _lease = initialization_lease;
            let prepared =
                Prepared::from_paths(&source, logos, &organizations).context(ResourceSnafu)?;
            Self::create_blocking(root, teams, Some(prepared))
        })
        .await
        .context(WorkerSnafu)?
    }
    async fn create_prepared(
        root: &Path,
        roster: &Roster,
        prepared: Option<Prepared>,
    ) -> Result<Self> {
        let root = root.to_owned();
        let teams = roster.teams().to_vec();
        tokio::task::spawn_blocking(move || Self::create_blocking(root, teams, prepared))
            .await
            .context(WorkerSnafu)?
    }
    fn create_blocking(
        root: PathBuf,
        teams: Vec<PhotoTeam>,
        prepared: Option<Prepared>,
    ) -> Result<Self> {
        let (root, database, writer) = storage::create(root).context(StorageSnafu)?;
        let cache = prepared
            .map(|p| p.copy_to(&root))
            .transpose()
            .context(ResourceSnafu)?
            .unwrap_or_default();
        database
            .save_roster_blocking(teams, cache)
            .context(PersistenceSnafu)?;
        Ok(Self {
            root,
            database,
            access: Writable(writer),
        })
    }
    pub(crate) async fn publish_verified(
        verified: WorkspaceReader,
        target: PathBuf,
        initialization_lease: std::sync::Arc<std::fs::File>,
    ) -> Result<Self> {
        tokio::task::spawn_blocking(move || {
            let _lease = initialization_lease;
            let Workspace {
                root,
                database,
                access: _,
            } = verified;
            // SQLite handles must be closed before moving the directory on Windows.
            drop(database);
            let (root, database, writer) =
                storage::publish_verified(root, &target).context(StorageSnafu)?;
            Ok(Self {
                root,
                database,
                access: Writable(writer),
            })
        })
        .await
        .context(WorkerSnafu)?
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
        };
        workspace.verify().await?;
        Ok(workspace)
    }
    pub(crate) fn writable_owner(&self) -> std::sync::Arc<std::fs::File> {
        std::sync::Arc::clone(&self.access.0)
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
        };
        workspace.verify().await?;
        Ok(workspace)
    }
}
impl<Access> Workspace<Access> {
    async fn verify(&self) -> Result<()> {
        let metadata = self.database.metadata().await.context(PersistenceSnafu)?;
        ensure!(matches!(metadata.ready, 0 | 1), InvalidReadinessSnafu);
        ensure!(metadata.ready == 1, IncompleteSnafu);
        self.roster().await?;
        self.verify_resources().await
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
