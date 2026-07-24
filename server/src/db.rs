use std::str::FromStr;

use snafu::{ResultExt, Snafu};
use sqlx::{
    SqlitePool,
    migrate::{MigrateError, Migrator},
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

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
}

/// Connects to `SQLite` and executes every pending embedded migration.
///
/// # Errors
///
/// Returns [`DatabaseError`] when the URL is invalid, the database cannot be opened or an
/// embedded migration fails.
pub async fn connect_and_migrate(database_url: &str) -> Result<SqlitePool, DatabaseError> {
    let options = SqliteConnectOptions::from_str(database_url).context(InvalidDatabaseUrlSnafu)?;
    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .context(ConnectSnafu)?;
    MIGRATOR.run(&pool).await.context(MigrateSnafu)?;
    Ok(pool)
}
