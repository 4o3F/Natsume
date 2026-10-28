//! Filesystem, connection and migration assembly, separate from workspace policy.
use super::db::Database;
use snafu::{ResultExt as _, Snafu, ensure};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Snafu)]
pub enum StorageError {
    #[snafu(display("workspace requires an absolute path without parent traversal"))]
    Path,
    #[snafu(display("workspace target already exists; no files were overwritten"))]
    Exists,
    #[snafu(display("workspace storage operation failed: {operation}"))]
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
    #[snafu(display("workspace database could not be opened or installed: {source}"))]
    Database { source: super::db::PersistenceError },
    #[snafu(display("workspace storage task failed"))]
    Worker { source: tokio::task::JoinError },
}
pub(super) type Result<T> = std::result::Result<T, StorageError>;
fn validate(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && !path.components().any(|c| c == Component::ParentDir),
        PathSnafu
    );
    Ok(())
}
pub(super) async fn create(path: PathBuf) -> Result<(PathBuf, Database)> {
    tokio::task::spawn_blocking(move || {
        validate(&path)?;
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return ExistsSnafu.fail();
            }
            Err(source) => {
                return Err(source).context(IoSnafu {
                    operation: "create workspace directory",
                });
            }
        }
        let root = std::fs::canonicalize(path).context(IoSnafu {
            operation: "resolve workspace directory",
        })?;
        let path = root.join("workspace.sqlite3");
        std::fs::File::create_new(&path).context(IoSnafu {
            operation: "reserve workspace database",
        })?;
        let database = Database::connect(&path).context(DatabaseSnafu)?;
        database.install().context(DatabaseSnafu)?;
        Ok((root, database))
    })
    .await
    .context(WorkerSnafu)?
}
pub(super) async fn open(path: PathBuf) -> Result<(PathBuf, Database)> {
    tokio::task::spawn_blocking(move || {
        validate(&path)?;
        let root = std::fs::canonicalize(path).context(IoSnafu {
            operation: "resolve workspace directory",
        })?;
        let path = root.join("workspace.sqlite3");
        let resolved = std::fs::canonicalize(&path).context(IoSnafu {
            operation: "resolve existing database",
        })?;
        ensure!(resolved == path && resolved.is_file(), PathSnafu);
        let database = Database::connect(&resolved).context(DatabaseSnafu)?;
        Ok((root, database))
    })
    .await
    .context(WorkerSnafu)?
}
