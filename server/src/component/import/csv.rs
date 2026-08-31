use std::{
    collections::HashSet,
    error::Error,
    fmt::{self, Display, Formatter},
    str,
};

use zeroize::{Zeroize, ZeroizeOnDrop};

use super::candidate::CandidateRowFacts;

const MAX_IMPORT_ROWS: usize = 10_000;

const CSV_HEADER: &str = "seat,account,password";
const UTF8_BOM: &[u8] = &[0xef, 0xbb, 0xbf];
const SEAT_CODE_LENGTH_LIMIT: usize = 64;
const ACCOUNT_USERNAME_LENGTH_LIMIT: usize = 64;
const PASSWORD_LENGTH_LIMIT: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CsvImportErrorCategory {
    InvalidUtf8,
    InvalidHeader,
    WrongColumnCount,
    EmptyField,
    FieldTooLong,
    ControlCharacter,
    DuplicateSeatCode,
    DuplicateAccountUsername,
    TooManyRows,
    ZeroDataRows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CsvImportError {
    line: usize,
    category: CsvImportErrorCategory,
}

impl CsvImportError {
    const fn new(line: usize, category: CsvImportErrorCategory) -> Self {
        Self { line, category }
    }

    #[cfg(test)]
    #[must_use]
    const fn line(&self) -> usize {
        self.line
    }

    #[must_use]
    pub(super) const fn category(&self) -> CsvImportErrorCategory {
        self.category
    }
}

impl Display for CsvImportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("the CSV import is invalid")
    }
}

impl Error for CsvImportError {}

#[derive(Zeroize, ZeroizeOnDrop)]
pub(super) struct ImportRow {
    seat_code: String,
    domjudge_username: String,
    password: String,
}

impl ImportRow {
    #[must_use]
    pub(super) fn seat_code(&self) -> &str {
        &self.seat_code
    }

    #[must_use]
    pub(super) fn domjudge_username(&self) -> &str {
        &self.domjudge_username
    }

    #[must_use]
    pub(super) fn password(&self) -> &str {
        &self.password
    }
}

pub(super) struct ParsedImport {
    rows: Vec<ImportRow>,
}

impl ParsedImport {
    #[must_use]
    pub(super) fn rows(&self) -> &[ImportRow] {
        &self.rows
    }

    pub(super) fn candidate_rows(&self) -> Vec<CandidateRowFacts> {
        self.rows
            .iter()
            .map(|row| CandidateRowFacts::new(row.seat_code.clone(), row.domjudge_username.clone()))
            .collect()
    }
}

/// Parses the frozen import CSV grammar and builds its canonical staging JSON.
///
/// # Errors
///
/// Returns a typed, content-free [`CsvImportError`] for the first invalid line.
///
/// # Panics
///
/// Panics only if serializing the fixed string-only staging shape violates its
/// infallible serialization invariant.
pub(super) fn parse_csv(raw: &[u8]) -> Result<ParsedImport, CsvImportError> {
    let bytes = raw.strip_prefix(UTF8_BOM).unwrap_or(raw);
    let text = str::from_utf8(bytes).map_err(|error| {
        let line = bytes[..error.valid_up_to()]
            .split(|byte| *byte == b'\n')
            .count();
        CsvImportError::new(line, CsvImportErrorCategory::InvalidUtf8)
    })?;

    let mut lines = text.split_inclusive('\n');
    let Some(header) = lines.next() else {
        return Err(CsvImportError::new(1, CsvImportErrorCategory::ZeroDataRows));
    };
    if csv_line(header) != CSV_HEADER {
        return Err(CsvImportError::new(
            1,
            CsvImportErrorCategory::InvalidHeader,
        ));
    }

    let mut rows = Vec::new();
    let mut seat_codes = HashSet::new();
    let mut account_usernames = HashSet::new();
    for (index, encoded_line) in lines.enumerate() {
        let line_number = index + 2;
        if rows.len() == MAX_IMPORT_ROWS {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::TooManyRows,
            ));
        }
        let line = csv_line(encoded_line);
        let fields = line.split(',').collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::WrongColumnCount,
            ));
        }
        if fields.iter().any(|field| field.is_empty()) {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::EmptyField,
            ));
        }
        if fields[0].len() > SEAT_CODE_LENGTH_LIMIT
            || fields[1].len() > ACCOUNT_USERNAME_LENGTH_LIMIT
            || fields[2].len() > PASSWORD_LENGTH_LIMIT
        {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::FieldTooLong,
            ));
        }
        if fields
            .iter()
            .any(|field| field.chars().any(char::is_control))
        {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::ControlCharacter,
            ));
        }
        if !seat_codes.insert(fields[0].to_owned()) {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::DuplicateSeatCode,
            ));
        }
        if !account_usernames.insert(fields[1].to_owned()) {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::DuplicateAccountUsername,
            ));
        }
        rows.push(ImportRow {
            seat_code: fields[0].to_owned(),
            domjudge_username: fields[1].to_owned(),
            password: fields[2].to_owned(),
        });
    }

    if rows.is_empty() {
        return Err(CsvImportError::new(1, CsvImportErrorCategory::ZeroDataRows));
    }
    Ok(ParsedImport { rows })
}

fn csv_line(encoded_line: &str) -> &str {
    encoded_line
        .strip_suffix('\n')
        .map_or(encoded_line, |line| line.strip_suffix('\r').unwrap_or(line))
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::{CsvImportErrorCategory, MAX_IMPORT_ROWS, parse_csv};

    struct RejectionCase {
        input: Vec<u8>,
        category: CsvImportErrorCategory,
        line: usize,
        canary: &'static str,
    }

    #[test]
    fn strict_parser_rejection_matrix_is_typed_and_redacted() {
        let mut invalid_utf8 = b"seat,account,password\nS-01,team-1,invalid-utf8-canary-".to_vec();
        invalid_utf8.push(0xff);
        let long_seat = format!(
            "seat,account,password\n{},team-1,field-length-canary",
            "S".repeat(65)
        )
        .into_bytes();
        let mut too_many_rows = String::from("seat,account,password");
        for index in 0..=MAX_IMPORT_ROWS {
            assert!(write!(too_many_rows, "\nS-{index},team-{index},row-cap-canary").is_ok());
        }
        let cases = [
            RejectionCase {
                input: invalid_utf8,
                category: CsvImportErrorCategory::InvalidUtf8,
                line: 2,
                canary: "invalid-utf8-canary",
            },
            RejectionCase {
                input: b"seat,account,wrong-header-canary".to_vec(),
                category: CsvImportErrorCategory::InvalidHeader,
                line: 1,
                canary: "wrong-header-canary",
            },
            RejectionCase {
                input: b"S-01,team-1,missing-header-canary".to_vec(),
                category: CsvImportErrorCategory::InvalidHeader,
                line: 1,
                canary: "missing-header-canary",
            },
            RejectionCase {
                input: b"seat,account,password\nS-01,team-1,column-count-canary,extra".to_vec(),
                category: CsvImportErrorCategory::WrongColumnCount,
                line: 2,
                canary: "column-count-canary",
            },
            RejectionCase {
                input: b"seat,account,password\n,team-1,empty-field-canary".to_vec(),
                category: CsvImportErrorCategory::EmptyField,
                line: 2,
                canary: "empty-field-canary",
            },
            RejectionCase {
                input: long_seat,
                category: CsvImportErrorCategory::FieldTooLong,
                line: 2,
                canary: "field-length-canary",
            },
            RejectionCase {
                input: b"seat,account,password\nS-01,team-1,control-canary\x07".to_vec(),
                category: CsvImportErrorCategory::ControlCharacter,
                line: 2,
                canary: "control-canary",
            },
            RejectionCase {
                input: b"seat,account,password\nS-01,team-1,first-password\nS-01,team-2,duplicate-seat-canary".to_vec(),
                category: CsvImportErrorCategory::DuplicateSeatCode,
                line: 3,
                canary: "duplicate-seat-canary",
            },
            RejectionCase {
                input: b"seat,account,password\nS-01,team-1,first-password\nS-02,team-1,duplicate-account-canary".to_vec(),
                category: CsvImportErrorCategory::DuplicateAccountUsername,
                line: 3,
                canary: "duplicate-account-canary",
            },
            RejectionCase {
                input: too_many_rows.into_bytes(),
                category: CsvImportErrorCategory::TooManyRows,
                line: MAX_IMPORT_ROWS + 2,
                canary: "row-cap-canary",
            },
            RejectionCase {
                input: Vec::new(),
                category: CsvImportErrorCategory::ZeroDataRows,
                line: 1,
                canary: "",
            },
            RejectionCase {
                input: b"seat,account,password\r\n".to_vec(),
                category: CsvImportErrorCategory::ZeroDataRows,
                line: 1,
                canary: "",
            },
        ];

        for case in cases {
            let Err(error) = parse_csv(&case.input) else {
                panic!("invalid CSV was accepted")
            };
            assert_eq!(error.category(), case.category);
            assert_eq!(error.line(), case.line);
            let display = error.to_string();
            let debug = format!("{error:?}");
            assert_eq!(display, "the CSV import is invalid");
            if !case.canary.is_empty() {
                assert!(!display.contains(case.canary));
                assert!(!debug.contains(case.canary));
            }
        }
    }

    #[test]
    fn parser_preserves_order() {
        let Ok(parsed) = parse_csv(
            b"seat,account,password\nB-02,team-beta,beta-password\nA-01,team-alpha,alpha-password",
        ) else {
            panic!("valid CSV was rejected")
        };
        assert_eq!(parsed.rows().len(), 2);
        assert_eq!(parsed.rows()[0].seat_code(), "B-02");
        assert_eq!(parsed.rows()[0].domjudge_username(), "team-beta");
        assert_eq!(parsed.rows()[0].password(), "beta-password");
        assert_eq!(parsed.rows()[1].seat_code(), "A-01");
        assert_eq!(parsed.rows()[1].domjudge_username(), "team-alpha");
    }

    #[test]
    fn parser_accepts_one_leading_bom_and_crlf_lines() {
        let Ok(with_bom) =
            parse_csv(b"\xef\xbb\xbfseat,account,password\nA-01,team-alpha,bom-password\n")
        else {
            panic!("BOM CSV was rejected")
        };
        assert_eq!(with_bom.rows()[0].password(), "bom-password");

        let Ok(with_crlf) =
            parse_csv(b"seat,account,password\r\nA-01,team-alpha,crlf-password\r\n")
        else {
            panic!("CRLF CSV was rejected")
        };
        assert_eq!(with_crlf.rows()[0].seat_code(), "A-01");
        assert_eq!(with_crlf.rows()[0].password(), "crlf-password");
    }
}
