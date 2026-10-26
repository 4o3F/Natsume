//! Local non-secret roster input. This does not grant server enrollment.
use std::{collections::BTreeSet, path::Path};

use serde::Deserialize;
use snafu::{ResultExt as _, Snafu, ensure};

/// The minimum team facts used by the offline client.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Team {
    pub account_id: String,
    /// External Team ID, sourced from the server's `DOMjudge` username.
    pub account: String,
    pub organization_id: i64,
    pub school_name_zh: String,
    #[serde(default)]
    pub school_name_en: String,
    pub team_name_zh: String,
    #[serde(default)]
    pub team_name_en: String,
}

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("无法读取名单文件"))]
    Read { source: std::io::Error },
    #[snafu(display("名单 JSON 格式或字段不符合要求"))]
    Json { source: serde_json::Error },
    #[snafu(display("名单不能为空"))]
    Empty,
    #[snafu(display("第 {row} 支队伍的 {field} 不能为空"))]
    MissingIdentity { row: usize, field: &'static str },
    #[snafu(display("第 {row} 支队伍的 {field} 与其他队伍重复"))]
    DuplicateIdentity { row: usize, field: &'static str },
}

pub type Result<T> = std::result::Result<T, Error>;

/// A validated roster. Its team list cannot be mutated through the public API.
#[derive(Debug)]
pub struct Roster {
    teams: Vec<Team>,
}

impl Roster {
    /// Reads and validates a local JSON array of teams.
    ///
    /// # Errors
    /// Returns a read, JSON, empty-roster, or identity validation error.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).context(ReadSnafu)?;
        Self::parse(&bytes)
    }

    /// Validates team fields and uniqueness without changing input identities.
    ///
    /// # Errors
    /// Rejects malformed/unknown fields, an empty roster, or empty/duplicate identities.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let teams: Vec<Team> = serde_json::from_slice(bytes).context(JsonSnafu)?;
        Self::from_teams(teams)
    }

    pub(crate) fn from_teams(teams: Vec<Team>) -> Result<Self> {
        ensure!(!teams.is_empty(), EmptySnafu);
        let mut account_ids = BTreeSet::new();
        let mut accounts = BTreeSet::new();
        for (index, team) in teams.iter().enumerate() {
            let row = index + 1;
            for (field, value, identities) in [
                ("account_id", team.account_id.as_str(), &mut account_ids),
                ("account", team.account.as_str(), &mut accounts),
            ] {
                ensure!(
                    !value.trim().is_empty(),
                    MissingIdentitySnafu { row, field }
                );
                ensure!(
                    identities.insert(value),
                    DuplicateIdentitySnafu { row, field }
                );
            }
        }
        Ok(Self { teams })
    }

    #[must_use]
    pub fn teams(&self) -> &[Team] {
        &self.teams
    }
}
