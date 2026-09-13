use std::{error::Error, fmt};

/// A workbook problem with an optional one-based worksheet cell location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputError {
    pub row: Option<u32>,
    pub column: Option<u32>,
    pub kind: InputErrorKind,
}

/// Content-free errors shared by the offline tool and import boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputErrorKind {
    WorkbookTooLarge,
    Archive,
    ExpandedWorkbookTooLarge,
    Workbook,
    MissingWorksheet,
    Worksheet,
    Header,
    MissingColumn(&'static str),
    TooManyRows,
    UnexpectedColumn,
    RepeatedCell,
    Formula,
    CellError,
    TextRequired,
    EmptyRoster,
    Required,
    FieldTooLong(usize),
    ControlCharacter,
    InvalidIdentifier,
    InvalidCountry,
    DuplicateAccount(u32),
    DuplicateSeat(u32),
    OrganizationConflict(u32),
}

impl InputError {
    pub(super) const fn file(kind: InputErrorKind) -> Self {
        Self {
            row: None,
            column: None,
            kind,
        }
    }

    pub(super) const fn cell(row: u32, column: u32, kind: InputErrorKind) -> Self {
        Self {
            row: Some(row),
            column: Some(column),
            kind,
        }
    }
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Teams")?;
        if let (Some(row), Some(column)) = (self.row, self.column) {
            write!(f, " row {row}, column {column}")?;
        }
        write!(f, ": {}", self.kind)
    }
}

impl fmt::Display for InputErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkbookTooLarge => f.write_str("XLSX exceeds the 8 MiB limit"),
            Self::Archive => f.write_str("cannot read the XLSX ZIP archive"),
            Self::ExpandedWorkbookTooLarge => f.write_str("expanded XLSX exceeds the 64 MiB limit"),
            Self::Workbook => f.write_str("cannot parse XLSX workbook metadata"),
            Self::MissingWorksheet => f.write_str("workbook must contain a worksheet named Teams"),
            Self::Worksheet => f.write_str("cannot parse worksheet cells"),
            Self::Header => f.write_str("row 1 must contain the template headers, once each"),
            Self::MissingColumn(name) => write!(f, "missing required header {name}"),
            Self::TooManyRows => f.write_str("data must fit within rows 2 through 10001"),
            Self::UnexpectedColumn => f.write_str("data outside the nine template columns"),
            Self::RepeatedCell => f.write_str("cell occurs more than once in worksheet XML"),
            Self::Formula => f.write_str("formulas are not allowed; paste values as text"),
            Self::CellError => f.write_str("Excel error cell; replace it with a text value"),
            Self::TextRequired => f.write_str("expected a text cell; numeric and date cells are not accepted"),
            Self::EmptyRoster => f.write_str("at least one team is required"),
            Self::Required => f.write_str("required text is empty"),
            Self::FieldTooLong(limit) => write!(f, "text exceeds the {limit} UTF-8 byte limit"),
            Self::ControlCharacter => f.write_str("control characters are not allowed"),
            Self::InvalidIdentifier => f.write_str("expected a CCS ID: 1-36 ASCII letters, digits, _, . or -; no leading . or -, no trailing ."),
            Self::InvalidCountry => f.write_str("country must be three uppercase ASCII letters, or empty for CHN"),
            Self::DuplicateAccount(row) => write!(f, "duplicate account; first used on row {row}"),
            Self::DuplicateSeat(row) => write!(f, "duplicate seat; first used on row {row}"),
            Self::OrganizationConflict(row) => write!(f, "conflicting school details; school first appears on row {row}"),
        }
    }
}

impl Error for InputError {}
