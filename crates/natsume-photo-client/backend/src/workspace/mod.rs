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
    #[snafu(display("capture requires a writable workspace owner"))]
    ReadOnly,
    #[snafu(display("workspace resource task failed"))]
    Worker { source: tokio::task::JoinError },
}
pub type Result<T> = std::result::Result<T, Error>;
pub struct Workspace {
    root: PathBuf,
    database: Database,
    writer: Option<std::sync::Arc<std::fs::File>>,
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
    ) -> Result<Self> {
        let organizations = roster
            .teams()
            .iter()
            .map(|team| team.organization_id)
            .collect();
        let prepared = tokio::task::spawn_blocking(move || {
            Prepared::from_paths(&source, logos, &organizations)
        })
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
        let (root, database, writer) = storage::create(root.to_owned())
            .await
            .context(StorageSnafu)?;
        let cache_root = root.clone();
        let cache_owner = std::sync::Arc::clone(&writer);
        let cache = tokio::task::spawn_blocking(move || {
            let _owner = cache_owner;
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
        Ok(Self {
            root,
            database,
            writer: Some(writer),
        })
    }
    /// Opens a prepared workspace without migrating or rewriting existing files.
    /// # Errors
    /// Rejects incomplete workspaces and damaged roster or logo data.
    pub async fn open(root: &Path) -> Result<Self> {
        let workspace = Self::open_for_verification(root, false).await?;
        workspace.verify_resources().await?;
        Ok(workspace)
    }
    /// Opens read-only metadata for diagnosis without installing migrations.
    /// Resource integrity must subsequently be checked by `initialization::diagnose`.
    /// # Errors
    /// Rejects incomplete or invalid persisted data.
    pub async fn open_readonly(root: &Path) -> Result<Self> {
        Self::open_for_verification(root, true).await
    }
    pub(crate) async fn open_for_verification(root: &Path, read_only: bool) -> Result<Self> {
        let (root, database, writer) = storage::open(root.to_owned(), read_only)
            .await
            .context(StorageSnafu)?;
        let metadata = database.metadata().await.context(PersistenceSnafu)?;
        ensure!(matches!(metadata.ready, 0 | 1), InvalidReadinessSnafu);
        ensure!(metadata.ready == 1, IncompleteSnafu);
        let workspace = Self {
            root,
            database,
            writer,
        };
        workspace.roster().await?;
        Ok(workspace)
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
    pub(crate) fn writable_owner(&self) -> Result<std::sync::Arc<std::fs::File>> {
        self.writer
            .as_ref()
            .map(std::sync::Arc::clone)
            .ok_or_else(|| ReadOnlySnafu.build())
    }
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}
