//! Local roster persistence. Readiness here is not server enrollment.
use std::path::{Component, Path, PathBuf};

use snafu::{IntoError as _, ResultExt as _, ensure};

use crate::roster::Roster;
use db::{Database, ExistsSnafu, IoSnafu, PathSnafu};

mod db;
mod resources;
pub use db::{Error, Result};

const DATABASE_NAME: &str = "workspace.sqlite3";

pub struct Workspace {
    root: PathBuf,
    database: Database,
}

impl Workspace {
    /// Creates a new local workspace from an already validated roster.
    ///
    /// # Errors
    /// Rejects unsafe paths, existing targets, or failed directory/database initialization.
    /// Failed initialization leaves its directory for inspection; it is never auto-deleted.
    pub fn create(root: &Path, roster: &Roster) -> Result<Self> {
        Self::create_prepared(root, roster, None)
    }

    /// Creates a workspace with local school logos and an optional default image.
    ///
    /// # Errors
    /// Rejects invalid manifests/images, unsafe paths, or failed file/database writes.
    pub fn create_with_resources(root: &Path, roster: &Roster, manifest: &Path) -> Result<Self> {
        let prepared = resources::Prepared::load(manifest, roster)?;
        Self::create_prepared(root, roster, Some(prepared))
    }

    fn create_prepared(
        root: &Path,
        roster: &Roster,
        prepared: Option<resources::Prepared>,
    ) -> Result<Self> {
        validate_path(root)?;
        match std::fs::create_dir(root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return ExistsSnafu.fail();
            }
            Err(source) => return Err(IoSnafu.into_error(source)),
        }
        let root = std::fs::canonicalize(root).context(IoSnafu)?;
        let path = root.join(DATABASE_NAME);
        // Reserve exactly this file. Neither create nor open may overwrite another file.
        std::fs::File::create_new(&path).context(IoSnafu)?;
        let mut database = Database::connect(&path)?;
        database.migrate()?;
        let cache = prepared
            .map(|prepared| prepared.copy_to(&root))
            .transpose()?
            .unwrap_or_default();
        database.initialize(roster, &cache)?;
        Ok(Self { root, database })
    }

    /// Opens a complete supported workspace and revalidates its persisted roster.
    ///
    /// # Errors
    /// Rejects missing/corrupt files, escaping paths, unsupported versions, incomplete
    /// initialization, or invalid persisted team data. It never creates a missing database.
    pub fn open(root: &Path) -> Result<Self> {
        validate_path(root)?;
        let root = std::fs::canonicalize(root).context(IoSnafu)?;
        let path = root.join(DATABASE_NAME);
        ensure!(path.is_file(), PathSnafu);
        let resolved = std::fs::canonicalize(&path).context(IoSnafu)?;
        ensure!(resolved.starts_with(&root), PathSnafu);
        let mut database = Database::connect(&resolved)?;
        database.check_ready()?;
        database.migrate()?;
        database.roster()?;
        for reference in database.resource_references()? {
            reference.verify(&root)?;
        }
        Ok(Self { root, database })
    }

    /// Reads a deterministic, validated roster ordered by external account.
    ///
    /// # Errors
    /// Returns a database error or a persisted-roster validation error.
    pub fn roster(&mut self) -> Result<Roster> {
        self.database.roster()
    }

    /// Gets the verified cached logo of a known team, if configured.
    /// # Errors
    /// Rejects an unknown team or invalid/missing cached image.
    pub fn logo_path(&mut self, account_id: &str) -> Result<Option<PathBuf>> {
        self.database
            .logo_reference(account_id)?
            .map(|image| image.verify(&self.root))
            .transpose()
    }

    /// Gets the verified default image, or None for the future built-in fallback.
    /// # Errors
    /// Returns a database or cached-image validation error.
    pub fn default_image_path(&mut self) -> Result<Option<PathBuf>> {
        self.database
            .default_reference()?
            .map(|image| image.verify(&self.root))
            .transpose()
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn validate_path(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && !path
                .components()
                .any(|part| matches!(part, Component::ParentDir)),
        PathSnafu
    );
    Ok(())
}
