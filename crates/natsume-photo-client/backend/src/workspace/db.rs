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

use super::final_photos::{ConfirmedPhoto, FinalDraft};
use super::resources::Cache;
use crate::diesel_schema::{photo_roster, photo_team_results, photo_workspace};
use crate::photo::{ConfirmationId, PhotoDimensions, PhotoFormat, ResultId, SubmissionId};
use natsume_photo_protocol::PhotoTeam;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");
const PATH_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'/').remove(b':');

#[derive(Debug, Snafu)]
pub enum PersistenceError {
    #[snafu(display("confirmed result identity or digest is invalid"))]
    FinalValue {
        source: natsume_photo_protocol::ValueError,
    },
    #[snafu(display("confirmed result number is invalid"))]
    FinalNumber { source: std::num::TryFromIntError },
    #[snafu(display("confirmed result metadata is invalid"))]
    FinalMetadata,
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
    Unavailable { source: std::sync::PoisonError<()> },
    #[snafu(display("workspace database task failed"))]
    Worker { source: tokio::task::JoinError },
}
pub(super) type Result<T> = std::result::Result<T, PersistenceError>;
pub(super) enum FinalCommit {
    Saved(ConfirmedPhoto),
    Conflict,
}

fn final_rows(tx: &mut Transaction<'_>, account: Option<&str>) -> Result<Vec<ConfirmedPhoto>> {
    type Row = (
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        String,
        i32,
        i32,
        i64,
    );
    let mut query = photo_team_results::table.into_boxed();
    if let Some(account) = account {
        query = query.filter(photo_team_results::account_id.eq(account));
    }
    let rows: Vec<Row> = query
        .select((
            photo_team_results::account_id,
            photo_team_results::confirmation_id,
            photo_team_results::result_id,
            photo_team_results::submission_id,
            photo_team_results::image_path,
            photo_team_results::image_sha256,
            photo_team_results::image_bytes,
            photo_team_results::image_format,
            photo_team_results::width,
            photo_team_results::height,
            photo_team_results::revision,
        ))
        .order(photo_team_results::account_id)
        .load(tx.connection())?;
    rows.into_iter()
        .map(
            |(
                account_id,
                confirmation,
                result,
                submission,
                path,
                sha256,
                bytes,
                format,
                width,
                height,
                revision,
            )| {
                let format = match format.as_str() {
                    "jpeg" => PhotoFormat::Jpeg,
                    "png" => PhotoFormat::Png,
                    _ => return FinalMetadataSnafu.fail(),
                };
                if revision < 1 {
                    return FinalMetadataSnafu.fail();
                }
                let _: natsume_photo_protocol::Sha256Digest =
                    sha256.parse().context(FinalValueSnafu)?;
                Ok(ConfirmedPhoto {
                    account_id,
                    confirmation_id: ConfirmationId::try_from(confirmation)
                        .context(FinalValueSnafu)?,
                    result_id: ResultId::try_from(result).context(FinalValueSnafu)?,
                    submission_id: SubmissionId::try_from(submission).context(FinalValueSnafu)?,
                    path: path.into(),
                    sha256,
                    bytes: u64::try_from(bytes).context(FinalNumberSnafu)?,
                    dimensions: PhotoDimensions {
                        width: u32::try_from(width).context(FinalNumberSnafu)?,
                        height: u32::try_from(height).context(FinalNumberSnafu)?,
                        format,
                    },
                    revision,
                })
            },
        )
        .collect()
}
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
pub(super) struct Database(
    Arc<Mutex<SqliteConnection>>,
    Option<super::WorkspaceWriter>,
    Arc<Mutex<()>>,
);
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
    pub(super) fn lock_final_operations(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        self.2
            .lock()
            .map_err(|_| UnavailableSnafu.into_error(std::sync::PoisonError::new(())))
    }
    pub(super) fn roster_ids(&self) -> Result<std::collections::BTreeSet<String>> {
        self.write_blocking(|tx| {
            let ids: Vec<String> = photo_roster::table
                .select(photo_roster::account_id)
                .load(tx.connection())?;
            Ok(ids.into_iter().collect())
        })
    }
    pub(super) fn final_photos(&self, account: Option<&str>) -> Result<Vec<ConfirmedPhoto>> {
        self.write_blocking(|tx| final_rows(tx, account))
    }
    pub(super) fn commit_final(
        &self,
        draft: &FinalDraft,
        path: std::path::PathBuf,
        baseline: i64,
    ) -> Result<FinalCommit> {
        self.write_blocking(|tx| {
            let current = final_rows(tx, Some(&draft.account_id))?.pop();
            if let Some(current) = &current
                && current.confirmation_id == draft.confirmation_id
            {
                return Ok(
                    if current.sha256 == draft.sha256
                        && current.bytes == draft.bytes
                        && current.dimensions == draft.dimensions
                    {
                        FinalCommit::Saved(current.clone())
                    } else {
                        FinalCommit::Conflict
                    },
                );
            }
            if current.as_ref().map_or(0, |photo| photo.revision) != baseline {
                return Ok(FinalCommit::Conflict);
            }
            let known = photo_roster::table
                .find(&draft.account_id)
                .select(photo_roster::account_id)
                .first::<String>(tx.connection())
                .optional()?;
            if known.is_none() {
                return Ok(FinalCommit::Conflict);
            }
            let Some(revision) = baseline.checked_add(1) else {
                return FinalMetadataSnafu.fail();
            };
            let photo = ConfirmedPhoto {
                account_id: draft.account_id.clone(),
                confirmation_id: draft.confirmation_id.clone(),
                result_id: ResultId::generate(),
                submission_id: SubmissionId::generate(),
                path,
                sha256: draft.sha256.clone(),
                bytes: draft.bytes,
                dimensions: draft.dimensions,
                revision,
            };
            let path = photo
                .path
                .to_str()
                .ok_or_else(|| FinalMetadataSnafu.build())?;
            let bytes = i64::try_from(photo.bytes).context(FinalNumberSnafu)?;
            let width = i32::try_from(photo.dimensions.width).context(FinalNumberSnafu)?;
            let height = i32::try_from(photo.dimensions.height).context(FinalNumberSnafu)?;
            let format = match photo.dimensions.format {
                PhotoFormat::Jpeg => "jpeg",
                PhotoFormat::Png => "png",
            };
            diesel::insert_into(photo_team_results::table)
                .values((
                    photo_team_results::account_id.eq(&photo.account_id),
                    photo_team_results::confirmation_id.eq(photo.confirmation_id.as_str()),
                    photo_team_results::result_id.eq(photo.result_id.as_str()),
                    photo_team_results::submission_id.eq(photo.submission_id.as_str()),
                    photo_team_results::image_path.eq(path),
                    photo_team_results::image_sha256.eq(&photo.sha256),
                    photo_team_results::image_bytes.eq(bytes),
                    photo_team_results::image_format.eq(format),
                    photo_team_results::width.eq(width),
                    photo_team_results::height.eq(height),
                    photo_team_results::revision.eq(revision),
                ))
                .on_conflict(photo_team_results::account_id)
                .do_update()
                .set((
                    photo_team_results::confirmation_id.eq(photo.confirmation_id.as_str()),
                    photo_team_results::result_id.eq(photo.result_id.as_str()),
                    photo_team_results::submission_id.eq(photo.submission_id.as_str()),
                    photo_team_results::image_path.eq(path),
                    photo_team_results::image_sha256.eq(&photo.sha256),
                    photo_team_results::image_bytes.eq(bytes),
                    photo_team_results::image_format.eq(format),
                    photo_team_results::width.eq(width),
                    photo_team_results::height.eq(height),
                    photo_team_results::revision.eq(revision),
                ))
                .execute(tx.connection())?;
            Ok(FinalCommit::Saved(photo))
        })
    }
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
            Ok(Self(
                Arc::new(Mutex::new(connection)),
                None,
                Arc::new(Mutex::new(())),
            ))
        })
    }
    pub(super) fn with_owner(mut self, owner: super::WorkspaceWriter) -> Self {
        self.1 = Some(owner);
        self
    }
    pub(super) fn install(&self) -> Result<()> {
        let mut connection = self.0.lock().map_err(|source| {
            drop(source.into_inner());
            UnavailableSnafu.into_error(std::sync::PoisonError::new(()))
        })?;
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
        let mut connection = self.0.lock().map_err(|source| {
            drop(source.into_inner());
            UnavailableSnafu.into_error(std::sync::PoisonError::new(()))
        })?;
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
