//! Local non-secret roster input. This does not grant server enrollment.
use std::{collections::BTreeSet, path::Path};

use snafu::{ResultExt as _, Snafu, ensure};

use natsume_photo_protocol::PhotoTeam;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("cannot read roster file"))]
    Read { source: std::io::Error },
    #[snafu(display("roster JSON format or fields are invalid"))]
    Json { source: serde_json::Error },
    #[snafu(display("roster must contain at least one team"))]
    Empty,
    #[snafu(display("team row {row} requires a nonempty {field}"))]
    MissingIdentity { row: usize, field: &'static str },
    #[snafu(display("team row {row} duplicates {field}"))]
    DuplicateIdentity { row: usize, field: &'static str },
}

pub type Result<T> = std::result::Result<T, Error>;

/// A validated roster. Its team list cannot be mutated through the public API.
#[derive(Debug)]
pub struct Roster {
    teams: Vec<PhotoTeam>,
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
        let teams: Vec<PhotoTeam> = serde_json::from_slice(bytes).context(JsonSnafu)?;
        Self::from_teams(teams)
    }

    pub(crate) fn from_teams(teams: Vec<PhotoTeam>) -> Result<Self> {
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
    pub fn teams(&self) -> &[PhotoTeam] {
        &self.teams
    }
}
