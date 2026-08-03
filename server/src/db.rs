use std::str::FromStr;

use snafu::{ResultExt, Snafu};
use sqlx::{
    Connection, SqlitePool,
    migrate::{MigrateError, Migrator},
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

pub mod domain_checks;
pub mod guarded;

/// Server-owned migration set embedded at compile time.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Snafu)]
pub enum DatabaseError {
    #[snafu(display("invalid SQLite database URL"))]
    InvalidDatabaseUrl { source: sqlx::Error },

    #[snafu(display("failed to connect to SQLite"))]
    Connect { source: sqlx::Error },

    #[snafu(display("failed to execute embedded SQLite migrations"))]
    Migrate { source: MigrateError },

    #[snafu(display("failed to acquire a SQLite connection for provisioning-window recovery"))]
    AcquireRecovery { source: sqlx::Error },

    #[snafu(display("failed to begin the provisioning-window recovery transaction"))]
    BeginRecovery { source: sqlx::Error },

    #[snafu(display("failed to read the provisioning-window singleton during recovery"))]
    RecoveryRead { source: guarded::GuardedWriteError },

    #[snafu(display("failed to write or verify the provisioning-window recovery audit"))]
    RecoveryAudit { source: guarded::GuardedWriteError },

    #[snafu(display(
        "failed to compare-and-swap the provisioning-window singleton during recovery"
    ))]
    RecoveryCompareAndSwap { source: guarded::GuardedWriteError },

    #[snafu(display("failed to commit provisioning-window recovery transaction"))]
    CommitRecovery { source: sqlx::Error },
}

/// Connects to `SQLite`, executes every pending embedded migration, and fails closed on recovery.
///
/// Recovery acquires a connection and starts `BEGIN IMMEDIATE` before observing the provisioning
/// singleton. An already closed window commits with no singleton or audit write; an open window is
/// atomically given exactly one recovery audit and compare-and-swap close.
///
/// # Errors
///
/// Returns [`DatabaseError`] when the URL is invalid, the database cannot be opened, migrations
/// fail, or a recovery transaction cannot be completed safely.
pub async fn connect_and_migrate(database_url: &str) -> Result<SqlitePool, DatabaseError> {
    let options = SqliteConnectOptions::from_str(database_url)
        .context(InvalidDatabaseUrlSnafu)?
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .context(ConnectSnafu)?;
    MIGRATOR.run(&pool).await.context(MigrateSnafu)?;

    let mut connection = pool.acquire().await.context(AcquireRecoverySnafu)?;
    let mut transaction = Connection::begin_with(&mut *connection, "BEGIN IMMEDIATE")
        .await
        .context(BeginRecoverySnafu)?;
    match guarded::close_provisioning_window_for_recovery(&mut transaction).await {
        Ok(_) => {}
        Err(guarded::RecoveryCloseError::Read { source }) => {
            return Err(DatabaseError::RecoveryRead { source });
        }
        Err(guarded::RecoveryCloseError::Audit { source }) => {
            return Err(DatabaseError::RecoveryAudit { source });
        }
        Err(guarded::RecoveryCloseError::CompareAndSwap { source }) => {
            return Err(DatabaseError::RecoveryCompareAndSwap { source });
        }
    }
    transaction.commit().await.context(CommitRecoverySnafu)?;
    Ok(pool)
}
