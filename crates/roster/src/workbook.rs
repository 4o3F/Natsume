use std::{
    collections::BTreeMap,
    io::{self, Cursor, Read},
};

use calamine::{DataRef, Reader, Xlsx};
use secrecy::{ExposeSecret, SecretString};
use zip::ZipArchive;

use crate::{InputError, InputErrorKind as Kind};

pub const MAX_WORKBOOK_BYTES: usize = 8 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DATA_ROWS: u32 = 10_000;
const HEADERS: [&str; 9] = [
    "organization_zh",
    "organization_en",
    "country",
    "account",
    "password",
    "seat",
    "team_name_zh",
    "team_name_en",
    "category",
];
type SheetRow = [Option<String>; HEADERS.len()];

/// A validated full roster. School order is lexicographic by its matching key.
#[derive(Debug)]
pub struct Roster {
    teams: Vec<Team>,
    organizations: Vec<Organization>,
}

impl Roster {
    #[must_use]
    pub fn teams(&self) -> &[Team] {
        &self.teams
    }

    #[must_use]
    pub fn organizations(&self) -> &[Organization] {
        &self.organizations
    }
}

/// School metadata derived from team rows; it has no assigned INST ID.
#[derive(Debug)]
pub struct Organization {
    name_zh: String,
    name_en: String,
    country: String,
    rows: Vec<u32>,
}

impl Organization {
    #[must_use]
    pub fn name_zh(&self) -> &str {
        &self.name_zh
    }
    #[must_use]
    pub fn name_en(&self) -> &str {
        &self.name_en
    }
    #[must_use]
    pub fn country(&self) -> &str {
        &self.country
    }
    #[must_use]
    pub fn rows(&self) -> &[u32] {
        &self.rows
    }
    #[must_use]
    pub fn key(&self) -> &str {
        if self.name_zh.is_empty() {
            &self.name_en
        } else {
            &self.name_zh
        }
    }
}

/// One team and its current credentials. Debug output redacts the password.
#[derive(Debug)]
pub struct Team {
    row: u32,
    organization_key: String,
    account: String,
    password: SecretString,
    seat: String,
    name_zh: String,
    name_en: String,
    category: String,
}

impl Team {
    #[must_use]
    pub const fn row(&self) -> u32 {
        self.row
    }
    #[must_use]
    pub fn organization_key(&self) -> &str {
        &self.organization_key
    }
    #[must_use]
    pub fn account(&self) -> &str {
        &self.account
    }
    #[must_use]
    pub fn password(&self) -> &str {
        self.password.expose_secret()
    }
    #[must_use]
    pub fn seat(&self) -> &str {
        &self.seat
    }
    #[must_use]
    pub fn name_zh(&self) -> &str {
        &self.name_zh
    }
    #[must_use]
    pub fn name_en(&self) -> &str {
        &self.name_en
    }
    #[must_use]
    pub fn category(&self) -> &str {
        &self.category
    }
}

/// Parse the fixed Teams template without evaluating formulas or coercing numbers.
///
/// # Errors
/// Returns a content-free error identifying the failing rule and cell when known.
pub fn parse_xlsx(bytes: &[u8]) -> Result<Roster, InputError> {
    check_archive(bytes)?;
    let mut workbook =
        Xlsx::new(Cursor::new(bytes)).map_err(|_| InputError::file(Kind::Workbook))?;
    if !workbook.sheet_names().iter().any(|name| name == "Teams") {
        return Err(InputError::file(Kind::MissingWorksheet));
    }
    let mut reader = workbook
        .worksheet_cells_reader("Teams")
        .map_err(|_| InputError::file(Kind::Worksheet))?;
    let mut rows: BTreeMap<u32, SheetRow> = BTreeMap::new();
    while let Some(cell) = reader
        .next_cell_with_formula_metadata()
        .map_err(|_| InputError::file(Kind::Worksheet))?
    {
        if cell.formula.is_none() && matches!(&cell.value, DataRef::Empty) {
            continue;
        }
        let (row, column) = (cell.pos.0.saturating_add(1), cell.pos.1.saturating_add(1));
        let error = |kind| InputError::cell(row, column, kind);
        if row > MAX_DATA_ROWS + 1 {
            return Err(error(Kind::TooManyRows));
        }
        let Some(slot) = usize::try_from(cell.pos.1)
            .ok()
            .filter(|&c| c < HEADERS.len())
        else {
            return Err(error(Kind::UnexpectedColumn));
        };
        if cell.formula.is_some() {
            return Err(error(Kind::Formula));
        }
        let value = match cell.value {
            DataRef::String(value) => value,
            DataRef::SharedString(value) => value.to_owned(),
            DataRef::Error(_) => return Err(error(Kind::CellError)),
            _ => return Err(error(Kind::TextRequired)),
        };
        if value.len() > 4096 {
            return Err(error(Kind::FieldTooLong(4096)));
        }
        let target = &mut rows.entry(row).or_default()[slot];
        if target.replace(value).is_some() {
            return Err(error(Kind::RepeatedCell));
        }
    }
    parse_rows(rows)
}

fn check_archive(bytes: &[u8]) -> Result<(), InputError> {
    if bytes.len() > MAX_WORKBOOK_BYTES {
        return Err(InputError::file(Kind::WorkbookTooLarge));
    }
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|_| InputError::file(Kind::Archive))?;
    let mut remaining = MAX_EXPANDED_BYTES;
    // Validate actual decompressed sizes before Calamine loads shared strings.
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|_| InputError::file(Kind::Archive))?;
        if entry.size() > remaining {
            return Err(InputError::file(Kind::ExpandedWorkbookTooLarge));
        }
        let consumed = io::copy(&mut entry.take(remaining + 1), &mut io::sink())
            .map_err(|_| InputError::file(Kind::Archive))?;
        remaining = remaining
            .checked_sub(consumed)
            .ok_or_else(|| InputError::file(Kind::ExpandedWorkbookTooLarge))?;
    }
    Ok(())
}

fn parse_rows(mut rows: BTreeMap<u32, SheetRow>) -> Result<Roster, InputError> {
    let header = rows
        .remove(&1)
        .ok_or_else(|| InputError::file(Kind::Header))?;
    let columns = header_columns(&header)?;
    let mut teams = Vec::new();
    let mut organizations: BTreeMap<String, Organization> = BTreeMap::new();
    let mut accounts = BTreeMap::new();
    let mut seats = BTreeMap::new();
    for (row, cells) in rows {
        if cells
            .iter()
            .all(|value| value.as_deref().unwrap_or_default().is_empty())
        {
            continue;
        }
        let mut fields: [String; HEADERS.len()] =
            std::array::from_fn(|i| cells[columns[i]].clone().unwrap_or_default());
        validate_fields(row, &columns, &mut fields)?;
        let [
            name_zh,
            name_en,
            country,
            account,
            password,
            seat,
            team_zh,
            team_en,
            category,
        ] = fields;
        if let Some(first) = accounts.insert(account.clone(), row) {
            return Err(field_error(row, &columns, 3, Kind::DuplicateAccount(first)));
        }
        if let Some(first) = seats.insert(seat.clone(), row) {
            return Err(field_error(row, &columns, 5, Kind::DuplicateSeat(first)));
        }
        let organization = Organization {
            name_zh,
            name_en,
            country,
            rows: vec![row],
        };
        let key = organization.key().to_owned();
        if let Some(existing) = organizations.get_mut(&key) {
            merge_organization(existing, organization, row, &columns)?;
        } else {
            organizations.insert(key.clone(), organization);
        }
        teams.push(Team {
            row,
            organization_key: key,
            account,
            password: password.into(),
            seat,
            name_zh: team_zh,
            name_en: team_en,
            category,
        });
    }
    if teams.is_empty() {
        return Err(InputError::file(Kind::EmptyRoster));
    }
    Ok(Roster {
        teams,
        organizations: organizations.into_values().collect(),
    })
}

fn header_columns(header: &SheetRow) -> Result<[usize; HEADERS.len()], InputError> {
    let mut columns = [None; HEADERS.len()];
    for (column, value) in header.iter().enumerate() {
        let name = value.as_deref().unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let Some(index) = HEADERS.iter().position(|&field| field == name) else {
            return Err(InputError::cell(
                1,
                u32::try_from(column + 1).unwrap_or(0),
                Kind::Header,
            ));
        };
        if columns[index].replace(column).is_some() {
            return Err(InputError::cell(
                1,
                u32::try_from(column + 1).unwrap_or(0),
                Kind::Header,
            ));
        }
    }
    for (index, column) in columns.iter().enumerate() {
        if column.is_none() {
            return Err(InputError::file(Kind::MissingColumn(HEADERS[index])));
        }
    }
    Ok(columns.map(Option::unwrap_or_default))
}

fn field_error(row: u32, columns: &[usize; 9], field: usize, kind: Kind) -> InputError {
    InputError::cell(row, u32::try_from(columns[field] + 1).unwrap_or(0), kind)
}

fn validate_fields(
    row: u32,
    columns: &[usize; 9],
    fields: &mut [String; 9],
) -> Result<(), InputError> {
    for (index, text) in fields.iter_mut().enumerate() {
        if text.chars().any(char::is_control) {
            return Err(field_error(row, columns, index, Kind::ControlCharacter));
        }
        // Credentials and seat codes retain their exact supplied text.
        if matches!(index, 0 | 1 | 2 | 6 | 7 | 8) {
            *text = text.trim().to_owned();
        }
        let limit = match index {
            3 | 8 => 36,
            4 => 512,
            5 => 64,
            _ => 1024,
        };
        if text.len() > limit {
            return Err(field_error(row, columns, index, Kind::FieldTooLong(limit)));
        }
    }
    for index in [3, 4, 5, 8] {
        if fields[index].is_empty() {
            return Err(field_error(row, columns, index, Kind::Required));
        }
    }
    for (primary, secondary) in [(0, 1), (6, 7)] {
        if fields[primary].is_empty() && fields[secondary].is_empty() {
            return Err(field_error(row, columns, primary, Kind::Required));
        }
    }
    for index in [3, 8] {
        if !valid_identifier(&fields[index]) {
            return Err(field_error(row, columns, index, Kind::InvalidIdentifier));
        }
    }
    if fields[2].is_empty() {
        "CHN".clone_into(&mut fields[2]);
    }
    if fields[2].len() != 3 || !fields[2].bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(field_error(row, columns, 2, Kind::InvalidCountry));
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 36
        && !value.starts_with(['.', '-'])
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn merge_organization(
    existing: &mut Organization,
    incoming: Organization,
    row: u32,
    columns: &[usize; 9],
) -> Result<(), InputError> {
    let error = |field| {
        field_error(
            row,
            columns,
            field,
            Kind::OrganizationConflict(existing.rows[0]),
        )
    };
    if existing.name_zh != incoming.name_zh {
        return Err(error(0));
    }
    if !existing.name_en.is_empty()
        && !incoming.name_en.is_empty()
        && existing.name_en != incoming.name_en
    {
        return Err(error(1));
    }
    if existing.country != incoming.country {
        return Err(error(2));
    }
    if existing.name_en.is_empty() {
        existing.name_en = incoming.name_en;
    }
    existing.rows.push(row);
    Ok(())
}

#[cfg(test)]
mod tests;
