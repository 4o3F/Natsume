//! Typed persistence only; workspace policy belongs to the enclosing component.
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use diesel::{
    Connection as _, ExpressionMethods as _, OptionalExtension as _, QueryDsl as _,
    RunQueryDsl as _, SqliteConnection,
};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness as _, embed_migrations};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use snafu::{IntoError as _, ResultExt as _, Snafu};

use super::resources::Cache;
use crate::diesel_schema::{photo_roster, photo_workspace};
use natsume_photo_protocol::PhotoTeam;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");
const PATH_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'/').remove(b':');

#[derive(Debug, Snafu)]
pub enum PersistenceError {
    #[snafu(display("cannot encode the workspace database path"))]
    Path,
    #[snafu(display("cannot connect to the existing workspace database"))]
    Connection { source: diesel::ConnectionError },
    #[snafu(display("cannot load the new workspace migration"))]
    MigrationSource {
        source: diesel_migrations::MigrationError,
    },
    #[snafu(display("cannot execute the new workspace migration"))]
    MigrationQuery { source: diesel::result::Error },
    #[snafu(display("workspace schema installation failed"))]
    MigrationFailed,
    #[snafu(display("workspace database query failed"))]
    Query { source: diesel::result::Error },
    #[snafu(display("workspace database executor is unavailable"))]
    Unavailable,
    #[snafu(display("workspace database task failed"))]
    Worker { source: tokio::task::JoinError },
}
pub(super) type Result<T> = std::result::Result<T, PersistenceError>;
impl From<diesel::result::Error> for PersistenceError {
    fn from(source: diesel::result::Error) -> Self {
        QuerySnafu.into_error(source)
    }
}

pub(super) struct Metadata {
    pub ready: i32,
}
pub(super) type LogoColumns = (Option<String>, Option<String>);
#[derive(Clone)]
pub(super) struct Database(Arc<Mutex<SqliteConnection>>, Option<Arc<std::fs::File>>);
pub(super) struct Transaction<'connection> {
    connection: &'connection mut SqliteConnection,
}
impl Transaction<'_> {
    pub(super) fn connection(&mut self) -> &mut SqliteConnection {
        self.connection
    }
}
#[derive(Clone, Copy)]
enum ConnectionMode {
    ReadOnly,
    Writable,
}
impl Database {
    // Storage invokes connection and schema installation only inside spawn_blocking.
    pub(super) fn connect(path: &Path) -> Result<Self> {
        Self::connect_mode(path, ConnectionMode::Writable)
    }
    pub(super) fn connect_readonly(path: &Path) -> Result<Self> {
        Self::connect_mode(path, ConnectionMode::ReadOnly)
    }
    fn connect_mode(path: &Path, mode: ConnectionMode) -> Result<Self> {
        let path = path.to_str().ok_or_else(|| PathSnafu.build())?;
        let encoded = utf8_percent_encode(path, PATH_ENCODE_SET);
        SqliteConnection::establish(&format!(
            "sqlite://{encoded}?mode={}",
            match mode {
                ConnectionMode::ReadOnly => "ro",
                ConnectionMode::Writable => "rw",
            }
        ))
        .context(ConnectionSnafu)
        .and_then(|mut connection| {
            use diesel::connection::SimpleConnection as _;
            // SQLite connection configuration; no typed Diesel PRAGMA API.
            connection
                .batch_execute("PRAGMA busy_timeout = 3000;")
                .context(QuerySnafu)?;
            Ok(Self(Arc::new(Mutex::new(connection)), None))
        })
    }
    pub(super) fn with_owner(mut self, owner: Arc<std::fs::File>) -> Self {
        self.1 = Some(owner);
        self
    }
    pub(super) fn install(&self) -> Result<()> {
        let mut connection = self.0.lock().map_err(|_| UnavailableSnafu.build())?;
        connection.run_pending_migrations(MIGRATIONS).map_err(|source| match source.downcast::<diesel::result::Error>() {
            Ok(source) => MigrationQuerySnafu.into_error(*source),
            Err(source) => match source.downcast::<diesel_migrations::MigrationError>() {
                Ok(source) => MigrationSourceSnafu.into_error(*source),
                Err(error) => {
                    tracing::error!(error = %snafu::Report::from_error(error.as_ref()), "embedded Photo schema installation failed");
                    MigrationFailedSnafu.build()
                }
            },
        })?;
        Ok(())
    }
    pub(super) async fn read<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Transaction<'_>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.write(operation).await
    }
    pub(super) async fn write<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Transaction<'_>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let database = self.clone();
        tokio::task::spawn_blocking(move || database.write_blocking(operation))
            .await
            .context(WorkerSnafu)?
    }
    fn write_blocking<T>(
        &self,
        operation: impl FnOnce(&mut Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut connection = self.0.lock().map_err(|_| UnavailableSnafu.build())?;
        connection.transaction(|connection| operation(&mut Transaction { connection }))
    }
    pub(super) async fn metadata(&self) -> Result<Metadata> {
        self.read(|tx| {
            let ready = photo_workspace::table
                .find(1)
                .select(photo_workspace::ready)
                .first(tx.connection())?;
            Ok(Metadata { ready })
        })
        .await
    }
    // Used only by the complete preparation operation on its blocking thread.
    pub(super) fn save_roster_blocking(&self, teams: Vec<PhotoTeam>, cache: Cache) -> Result<()> {
        self.write_blocking(move |tx| {
            for team in teams {
                let logo = cache.logos.get(&team.organization_id);
                diesel::insert_into(photo_roster::table)
                    .values((
                        photo_roster::account_id.eq(team.account_id),
                        photo_roster::account.eq(team.account),
                        photo_roster::organization_id.eq(team.organization_id),
                        photo_roster::school_name_zh.eq(team.school_name_zh),
                        photo_roster::school_name_en.eq(team.school_name_en),
                        photo_roster::team_name_zh.eq(team.team_name_zh),
                        photo_roster::team_name_en.eq(team.team_name_en),
                        photo_roster::logo_path.eq(logo.map(|l| l.path.as_str())),
                        photo_roster::logo_sha256.eq(logo.map(|l| l.sha256.as_str())),
                    ))
                    .execute(tx.connection())?;
            }
            diesel::update(photo_workspace::table.find(1))
                .set(photo_workspace::ready.eq(1))
                .execute(tx.connection())?;
            Ok(())
        })
    }
    pub(super) async fn logo(&self, account_id: String) -> Result<Option<LogoColumns>> {
        self.read(move |tx| {
            photo_roster::table
                .find(account_id)
                .select((photo_roster::logo_path, photo_roster::logo_sha256))
                .first(tx.connection())
                .optional()
                .map_err(PersistenceError::from)
        })
        .await
    }
    pub(super) async fn logo_entries(
        &self,
    ) -> Result<Vec<(String, Option<String>, Option<String>)>> {
        self.read(|tx| {
            photo_roster::table
                .select((
                    photo_roster::account_id,
                    photo_roster::logo_path,
                    photo_roster::logo_sha256,
                ))
                .load(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await
    }
    pub(super) async fn logos(&self) -> Result<Vec<LogoColumns>> {
        self.read(|tx| {
            photo_roster::table
                .select((photo_roster::logo_path, photo_roster::logo_sha256))
                .load(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await
    }
    pub(super) async fn teams(&self) -> Result<Vec<PhotoTeam>> {
        self.read(|tx| {
            type Row = (String, String, i64, String, String, String, String);
            let rows: Vec<Row> = photo_roster::table
                .select((
                    photo_roster::account_id,
                    photo_roster::account,
                    photo_roster::organization_id,
                    photo_roster::school_name_zh,
                    photo_roster::school_name_en,
                    photo_roster::team_name_zh,
                    photo_roster::team_name_en,
                ))
                .order(photo_roster::account)
                .load(tx.connection())?;
            Ok(rows
                .into_iter()
                .map(
                    |(
                        account_id,
                        account,
                        organization_id,
                        school_name_zh,
                        school_name_en,
                        team_name_zh,
                        team_name_en,
                    )| PhotoTeam {
                        account_id,
                        account,
                        organization_id,
                        school_name_zh,
                        school_name_en,
                        team_name_zh,
                        team_name_en,
                    },
                )
                .collect())
        })
        .await
    }
}
#[cfg(test)]
#[path = "../../tests/unit/workspace_db.rs"]
mod tests;
