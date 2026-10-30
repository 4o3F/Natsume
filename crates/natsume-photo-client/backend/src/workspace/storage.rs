//! Filesystem, connection and migration assembly, separate from workspace policy.
use super::{WorkspaceWriter, db::Database};
use snafu::{ResultExt as _, Snafu, ensure};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Snafu)]
pub enum StorageError {
    #[snafu(display("workspace requires an absolute path without parent traversal"))]
    Path,
    #[snafu(display("workspace already has a writable owner"))]
    Busy,
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
// The caller owns the blocking boundary for the entire preparation transaction.
pub(super) fn create(path: PathBuf) -> Result<(PathBuf, Database, WorkspaceWriter)> {
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
    let lock = lock_writer(&root)?;
    let path = root.join("workspace.sqlite3");
    std::fs::File::create_new(&path).context(IoSnafu {
        operation: "reserve workspace database",
    })?;
    let database = Database::connect(&path)
        .context(DatabaseSnafu)?
        .with_owner(lock.clone());
    database.install().context(DatabaseSnafu)?;
    Ok((root, database, lock))
}
fn existing_database(path: PathBuf) -> Result<(PathBuf, PathBuf)> {
    validate(&path)?;
    let root = std::fs::canonicalize(path).context(IoSnafu {
        operation: "resolve workspace directory",
    })?;
    let path = root.join("workspace.sqlite3");
    let resolved = std::fs::canonicalize(&path).context(IoSnafu {
        operation: "resolve existing database",
    })?;
    ensure!(resolved == path && resolved.is_file(), PathSnafu);
    Ok((root, resolved))
}
pub(super) async fn open_reader(path: PathBuf) -> Result<(PathBuf, Database)> {
    tokio::task::spawn_blocking(move || {
        let (root, path) = existing_database(path)?;
        let database = Database::connect_readonly(&path).context(DatabaseSnafu)?;
        Ok((root, database))
    })
    .await
    .context(WorkerSnafu)?
}
pub(super) async fn open_writer(path: PathBuf) -> Result<(PathBuf, Database, WorkspaceWriter)> {
    tokio::task::spawn_blocking(move || {
        let (root, path) = existing_database(path)?;
        let owner = lock_writer(&root)?;
        let database = Database::connect(&path)
            .context(DatabaseSnafu)?
            .with_owner(owner.clone());
        Ok((root, database, owner))
    })
    .await
    .context(WorkerSnafu)?
}

// The caller consumes an inspected workspace and runs this under its initialization lease.
pub(super) fn publish_verified(
    source: PathBuf,
    target: &Path,
) -> Result<(PathBuf, Database, WorkspaceWriter)> {
    let (root, _) = existing_database(source)?;
    validate(target)?;
    let parent = target.parent().ok_or_else(|| PathSnafu.build())?;
    let name = target.file_name().ok_or_else(|| PathSnafu.build())?;
    let target = std::fs::canonicalize(parent)
        .context(IoSnafu {
            operation: "resolve publication parent",
        })?
        .join(name);
    let mut owner = lock_writer(&root)?;
    if root != target {
        ensure!(
            !target.try_exists().context(IoSnafu {
                operation: "check publication target"
            })?,
            ExistsSnafu
        );
        // Windows cannot move the directory while its lock file is open.
        // The initialization lease still excludes other publication operations;
        // a writer winning the target afterwards is rejected by lock_writer.
        drop(owner);
        std::fs::rename(&root, &target).context(IoSnafu {
            operation: "publish verified workspace",
        })?;
        owner = lock_writer(&target)?;
    }
    let database = Database::connect(&target.join("workspace.sqlite3"))
        .context(DatabaseSnafu)?
        .with_owner(owner.clone());
    Ok((target, database, owner))
}

fn lock_writer(root: &Path) -> Result<WorkspaceWriter> {
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".writer.lock"))
        .context(IoSnafu {
            operation: "open workspace lock",
        })?;
    match lock.try_lock() {
        Ok(()) => Ok(WorkspaceWriter {
            _handle: std::sync::Arc::new(lock),
        }),
        Err(std::fs::TryLockError::WouldBlock) => BusySnafu.fail(),
        Err(std::fs::TryLockError::Error(source)) => Err(source).context(IoSnafu {
            operation: "lock workspace",
        }),
    }
}
