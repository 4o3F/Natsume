use std::path::Path;

use diesel::{
    Connection as _, ExpressionMethods as _, OptionalExtension as _, QueryDsl as _,
    RunQueryDsl as _, SqliteConnection,
};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness as _, embed_migrations};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use snafu::{IntoError as _, ResultExt as _, Snafu, ensure};

use super::resources::{Cache, ImageRef};
use crate::{
    diesel_schema::{photo_roster, photo_workspace},
    roster::{Roster, Team},
};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");
const FORMAT_VERSION: i32 = 1;
const PATH_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'/').remove(b':');

#[cfg(test)]
mod tests;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub enum Error {
    #[snafu(display("无法访问本地工作区文件"))]
    Io { source: std::io::Error },
    #[snafu(display("工作区必须使用有效绝对路径，且不能越出指定目录"))]
    Path,
    #[snafu(display("工作区目标已经存在，不会覆盖"))]
    Exists,
    #[snafu(display("无法连接本地数据库"))]
    Connection { source: diesel::ConnectionError },
    #[snafu(display("本地数据库迁移失败"))]
    Migration,
    #[snafu(display("本地数据库操作失败"))]
    Database { source: diesel::result::Error },
    #[snafu(display("不支持工作区格式版本 {version}"))]
    Version { version: i32 },
    #[snafu(display("工作区准备未完成，不能使用"))]
    Incomplete,
    #[snafu(display("持久化名单验证失败：{source}"))]
    Roster { source: crate::roster::Error },
    #[snafu(display("展示资源清单格式或字段不符合要求"))]
    ResourcesJson { source: serde_json::Error },
    #[snafu(display("展示资源配置、哈希、大小或格式无效"))]
    Resource,
    #[snafu(display("展示图片无法解码或超过像素限额"))]
    Decode { source: image::ImageError },
    #[snafu(display("队伍不存在"))]
    UnknownTeam,
}
pub type Result<T> = std::result::Result<T, Error>;

impl From<diesel::result::Error> for Error {
    fn from(source: diesel::result::Error) -> Self {
        DatabaseSnafu.into_error(source)
    }
}

pub(super) struct Database(SqliteConnection);
struct Transaction<'a> {
    connection: &'a mut SqliteConnection,
}
impl Transaction<'_> {
    fn connection(&mut self) -> &mut SqliteConnection {
        self.connection
    }
}

impl Database {
    pub(super) fn connect(path: &Path) -> Result<Self> {
        let path = path.to_str().ok_or_else(|| PathSnafu.build())?;
        // Escape URI metacharacters (including Windows' canonical-path '?').
        // mode=rw prevents SQLite from silently creating a missing file.
        let encoded = utf8_percent_encode(path, PATH_ENCODE_SET);
        let url = format!("sqlite://{encoded}?mode=rw");
        SqliteConnection::establish(&url)
            .map(Self)
            .context(ConnectionSnafu)
    }

    pub(super) fn migrate(&mut self) -> Result<()> {
        self.0
            .run_pending_migrations(MIGRATIONS)
            .map_err(|_| MigrationSnafu.build())?;
        Ok(())
    }

    fn read<T>(&mut self, operation: impl FnOnce(&mut Transaction<'_>) -> Result<T>) -> Result<T> {
        self.write(operation)
    }

    fn write<T>(&mut self, operation: impl FnOnce(&mut Transaction<'_>) -> Result<T>) -> Result<T> {
        self.0
            .transaction(|connection| operation(&mut Transaction { connection }))
    }

    pub(super) fn check_ready(&mut self) -> Result<()> {
        let (version, ready): (i32, bool) = self.read(|transaction| {
            photo_workspace::table
                .find(1)
                .select((photo_workspace::format_version, photo_workspace::ready))
                .first(transaction.connection())
                .context(DatabaseSnafu)
        })?;
        ensure!(version == FORMAT_VERSION, VersionSnafu { version });
        ensure!(ready, IncompleteSnafu);
        Ok(())
    }

    pub(super) fn initialize(&mut self, roster: &Roster, cache: &Cache) -> Result<()> {
        self.write(|transaction| {
            for team in roster.teams() {
                let logo = cache.logos.get(&team.organization_id);
                diesel::insert_into(photo_roster::table)
                    .values((
                        photo_roster::account_id.eq(&team.account_id),
                        photo_roster::account.eq(&team.account),
                        photo_roster::organization_id.eq(team.organization_id),
                        photo_roster::school_name_zh.eq(&team.school_name_zh),
                        photo_roster::school_name_en.eq(&team.school_name_en),
                        photo_roster::team_name_zh.eq(&team.team_name_zh),
                        photo_roster::team_name_en.eq(&team.team_name_en),
                        photo_roster::logo_path.eq(logo.map(|image| image.path.as_str())),
                        photo_roster::logo_sha256.eq(logo.map(|image| image.sha256.as_str())),
                    ))
                    .execute(transaction.connection())
                    .context(DatabaseSnafu)?;
            }
            diesel::update(photo_workspace::table.find(1))
                .set((
                    photo_workspace::ready.eq(true),
                    photo_workspace::default_image_path.eq(cache
                        .default_image
                        .as_ref()
                        .map(|image| image.path.as_str())),
                    photo_workspace::default_image_sha256.eq(cache
                        .default_image
                        .as_ref()
                        .map(|image| image.sha256.as_str())),
                ))
                .execute(transaction.connection())
                .context(DatabaseSnafu)?;
            Ok(())
        })
    }

    pub(super) fn logo_reference(&mut self, account_id: &str) -> Result<Option<ImageRef>> {
        let row: Option<(Option<String>, Option<String>)> = self.read(|transaction| {
            photo_roster::table
                .find(account_id)
                .select((photo_roster::logo_path, photo_roster::logo_sha256))
                .first(transaction.connection())
                .optional()
                .context(DatabaseSnafu)
        })?;
        let (path, hash) = row.ok_or_else(|| UnknownTeamSnafu.build())?;
        ImageRef::from_columns(path, hash)
    }

    pub(super) fn default_reference(&mut self) -> Result<Option<ImageRef>> {
        let (path, hash): (Option<String>, Option<String>) = self.read(|transaction| {
            photo_workspace::table
                .find(1)
                .select((
                    photo_workspace::default_image_path,
                    photo_workspace::default_image_sha256,
                ))
                .first(transaction.connection())
                .context(DatabaseSnafu)
        })?;
        ImageRef::from_columns(path, hash)
    }

    pub(super) fn resource_references(&mut self) -> Result<std::collections::BTreeSet<ImageRef>> {
        let rows: Vec<(Option<String>, Option<String>)> = self.read(|transaction| {
            photo_roster::table
                .select((photo_roster::logo_path, photo_roster::logo_sha256))
                .load(transaction.connection())
                .context(DatabaseSnafu)
        })?;
        let mut references = std::collections::BTreeSet::new();
        for (path, hash) in rows {
            if let Some(reference) = ImageRef::from_columns(path, hash)? {
                references.insert(reference);
            }
        }
        if let Some(reference) = self.default_reference()? {
            references.insert(reference);
        }
        Ok(references)
    }

    pub(super) fn roster(&mut self) -> Result<Roster> {
        self.read(|transaction| {
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
                .load(transaction.connection())
                .context(DatabaseSnafu)?;
            let teams = rows
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
                    )| Team {
                        account_id,
                        account,
                        organization_id,
                        school_name_zh,
                        school_name_en,
                        team_name_zh,
                        team_name_en,
                    },
                )
                .collect();
            Roster::from_teams(teams).context(RosterSnafu)
        })
    }
}
