//! Workspace business rules, separate from storage assembly and typed queries.
use crate::roster::Roster;
use db::Database;
use resources::{ImageRef, Prepared};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

mod db;
mod resources;
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
pub struct Workspace {
    root: PathBuf,
    database: Database,
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
    async fn create_prepared(
        root: &Path,
        roster: &Roster,
        prepared: Option<Prepared>,
    ) -> Result<Self> {
        let (root, database) = storage::create(root.to_owned())
            .await
            .context(StorageSnafu)?;
        let cache_root = root.clone();
        let cache = tokio::task::spawn_blocking(move || {
            prepared
                .map(|p| p.copy_to(&cache_root))
                .transpose()
                .map(Option::unwrap_or_default)
        })
        .await
        .context(WorkerSnafu)?
        .context(ResourceSnafu)?;
        database
            .save_roster(roster.teams().to_vec(), cache)
            .await
            .context(PersistenceSnafu)?;
        Ok(Self { root, database })
    }
    /// Opens a prepared workspace without migrating or rewriting existing files.
    /// # Errors
    /// Rejects incomplete workspaces and damaged roster or logo data.
    pub async fn open(root: &Path) -> Result<Self> {
        let (root, database) = storage::open(root.to_owned()).await.context(StorageSnafu)?;
        let metadata = database.metadata().await.context(PersistenceSnafu)?;
        ensure!(matches!(metadata.ready, 0 | 1), InvalidReadinessSnafu);
        ensure!(metadata.ready == 1, IncompleteSnafu);
        let workspace = Self { root, database };
        workspace.roster().await?;
        let rows = workspace.database.logos().await.context(PersistenceSnafu)?;
        let references: BTreeSet<_> = rows
            .into_iter()
            .map(|(path, hash)| ImageRef::from_columns(path, hash))
            .collect::<std::result::Result<Vec<_>, _>>()
            .context(ResourceSnafu)?
            .into_iter()
            .flatten()
            .collect();
        let root = workspace.root.clone();
        tokio::task::spawn_blocking(move || {
            for reference in references {
                reference.verify(&root)?;
            }
            Ok(())
        })
        .await
        .context(WorkerSnafu)?
        .context(ResourceSnafu)?;
        Ok(workspace)
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
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}
