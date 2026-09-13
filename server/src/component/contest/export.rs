use std::{
    collections::BTreeSet,
    fmt::Write as _,
    io::{self, Cursor, Seek, SeekFrom, Write},
};

use natsume_roster::{LogoMatch, read_logo};
use serde::Serialize;
use snafu::Snafu;
use zeroize::{Zeroize, Zeroizing};
use zip::{ZipWriter, write::SimpleFileOptions};

use super::{ContestComponent, db, logos, roster::ExportRoster};
use crate::{db::PersistenceError, vault::VaultSession};

const MAX_EXPORT_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Snafu)]
pub(crate) enum ExportError {
    #[snafu(display("Another image or export operation is running; retry shortly"))]
    Busy,
    #[snafu(display("Import a complete roster before exporting"))]
    IncompleteRoster,
    #[snafu(display("The committed roster could not be read"))]
    Persistence,
    #[snafu(display("The current account credentials could not be decrypted"))]
    Vault,
    #[snafu(display("School logo is missing or ambiguous"))]
    LogoUnavailable,
    #[snafu(display(
        "Cannot read storage.organization_logos; check the directory and service user permissions"
    ))]
    LogoDirectory,
    #[snafu(display("{organization}: {file}: {reason}"))]
    Logo {
        organization: String,
        file: String,
        reason: String,
    },
    #[snafu(display("Export encoding failed or the ZIP exceeds 256 MiB"))]
    Archive,
    #[snafu(display("Export worker failed"))]
    Worker,
}

impl ContestComponent {
    pub(crate) async fn export_domjudge(&self) -> Result<Vec<u8>, ExportError> {
        let permit = self
            .export_work
            .clone()
            .try_acquire_owned()
            .map_err(|_| ExportError::Busy)?;
        let roster = self
            .database
            .read(db::export_roster)
            .await
            .map_err(|error| match error.into_error() {
                PersistenceError::InvalidPersistedData => ExportError::IncompleteRoster,
                PersistenceError::OperationFailed => ExportError::Persistence,
            })?;
        let vault = self.vault.clone();
        let directory = self.logo_directory.clone();
        // The read transaction has ended; conversion and plaintext serialization
        // run on one bounded worker, outside both SQLite and the device loop.
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            build(&roster, &vault, &directory)
        })
        .await
        .map_err(|_| ExportError::Worker)?
    }
}

#[derive(Serialize)]
struct Group<'a> {
    id: &'a str,
    name: &'a str,
}
#[derive(Serialize)]
struct Organization<'a> {
    id: String,
    name: &'a str,
    formal_name: &'a str,
    country: &'a str,
}
#[derive(Serialize)]
struct Team<'a> {
    id: &'a str,
    name: String,
    display_name: String,
    group_ids: [&'a str; 1],
    organization_id: String,
    label: &'a str,
    location: Location<'a>,
}
#[derive(Serialize)]
struct Location<'a> {
    description: &'a str,
}
#[derive(Serialize)]
struct Account<'a> {
    id: &'a str,
    username: &'a str,
    name: &'a str,
    team_id: &'a str,
    r#type: &'static str,
    password: &'a str,
}

fn build(
    roster: &ExportRoster,
    vault: &VaultSession,
    directory: &std::path::Path,
) -> Result<Vec<u8>, ExportError> {
    if roster.teams.is_empty() {
        return Err(ExportError::IncompleteRoster);
    }
    let groups: Vec<_> = roster
        .teams
        .iter()
        .map(|t| t.category.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|id| Group { id, name: id })
        .collect();
    let organizations: Vec<_> = roster
        .organizations
        .iter()
        .map(|o| Organization {
            id: format!("INST-{:03}", o.organization_id),
            name: o.key(),
            formal_name: o.key(),
            country: &o.country,
        })
        .collect();
    let teams: Vec<_> = roster
        .teams
        .iter()
        .map(|t| {
            let name = match (t.name_zh.is_empty(), t.name_en.is_empty()) {
                (true, _) => t.name_en.clone(),
                (_, true) => t.name_zh.clone(),
                _ => format!("{}({})", t.name_zh, t.name_en),
            };
            Team {
                id: &t.account,
                display_name: name.clone(),
                name,
                group_ids: [&t.category],
                organization_id: format!("INST-{:03}", t.organization_id),
                label: &t.seat,
                location: Location {
                    description: &t.seat,
                },
            }
        })
        .collect();
    let mut zip = ZipWriter::new(LimitedArchive(Cursor::new(Vec::new())));
    add_json(&mut zip, "groups.json", &groups)?;
    add_json(&mut zip, "organizations.json", &organizations)?;
    add_json(&mut zip, "teams.json", &teams)?;
    let passwords = roster
        .teams
        .iter()
        .map(|t| {
            vault
                .open(&t.nonce, &t.ciphertext)
                .map_err(|_| ExportError::Vault)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let accounts = roster
        .teams
        .iter()
        .zip(&passwords)
        .map(|(t, password)| {
            Ok(Account {
                id: &t.account,
                username: &t.account,
                name: &t.account,
                team_id: &t.account,
                r#type: "team",
                password: std::str::from_utf8(password).map_err(|_| ExportError::Vault)?,
            })
        })
        .collect::<Result<Vec<_>, ExportError>>()?;
    let yaml =
        Zeroizing::new(serde_saphyr::to_string(&accounts).map_err(|_| ExportError::Archive)?);
    add(&mut zip, "accounts.yaml", yaml.as_bytes())?;
    drop(yaml);
    drop(accounts);
    drop(passwords);

    add_logos(&mut zip, roster, directory)?;
    let mut archive = zip.finish().map_err(|_| ExportError::Archive)?;
    Ok(std::mem::take(archive.0.get_mut()))
}

fn add_logos(
    zip: &mut ZipWriter<LimitedArchive>,
    roster: &ExportRoster,
    directory: &std::path::Path,
) -> Result<(), ExportError> {
    let index = logos::open_directory(directory)?;
    let mut readme = include_str!("export-readme.md").to_owned();
    readme.push_str("\n| Organization | Chinese name | English name | Source image / status |\n| --- | --- | --- | --- |\n");
    for school in &roster.organizations {
        let status = match logos::resolve(index.as_ref(), school) {
            LogoMatch::Missing => "Missing: add an image named after the school".to_owned(),
            LogoMatch::Ambiguous(paths) => format!(
                "Ambiguous: {}",
                paths
                    .iter()
                    .map(|p| logos::filename(p))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            LogoMatch::Unique(path) => {
                let png = read_logo(&path)
                    .and_then(natsume_roster::DecodedLogo::into_png)
                    .map_err(|error| logos::logo_error(school, &path, &error))?;
                add(
                    zip,
                    &format!("logos/INST-{:03}.png", school.organization_id),
                    &png,
                )?;
                logos::filename(&path)
            }
        };
        writeln!(
            readme,
            "| INST-{:03} | {} | {} | {} |",
            school.organization_id,
            markdown_cell(&school.name_zh),
            markdown_cell(&school.name_en),
            markdown_cell(&status)
        )
        .map_err(|_| ExportError::Archive)?;
    }
    add(zip, "README.md", readme.as_bytes())?;
    Ok(())
}

fn markdown_cell(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace(['\r', '\n'], " ")
}

fn add_json<T: Serialize>(
    zip: &mut ZipWriter<LimitedArchive>,
    name: &str,
    value: &T,
) -> Result<(), ExportError> {
    add(
        zip,
        name,
        &serde_json::to_vec_pretty(value).map_err(|_| ExportError::Archive)?,
    )
}
fn add(zip: &mut ZipWriter<LimitedArchive>, name: &str, bytes: &[u8]) -> Result<(), ExportError> {
    zip.start_file(
        name,
        SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o600),
    )
    .map_err(|_| ExportError::Archive)?;
    zip.write_all(bytes).map_err(|_| ExportError::Archive)
}

// Limit total in-memory output, including ZIP metadata. Erase aborted plaintext archives.
struct LimitedArchive(Cursor<Vec<u8>>);
impl Drop for LimitedArchive {
    fn drop(&mut self) {
        self.0.get_mut().zeroize();
    }
}
impl Write for LimitedArchive {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.position().saturating_add(bytes.len() as u64) > MAX_EXPORT_BYTES {
            return Err(io::Error::other("export size limit"));
        }
        self.0.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
impl Seek for LimitedArchive {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.seek(position)
    }
}

#[cfg(test)]
mod tests;
