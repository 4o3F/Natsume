use std::{
    fmt::Write as _,
    io::{Cursor, Read, Write},
};

use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

use super::*;

const EXAMPLE: &[u8] = include_bytes!("../../examples/roster.xlsx");

fn checked<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("test setup failed: {error:?}"))
}

fn rejected(bytes: &[u8]) -> InputError {
    match parse_xlsx(bytes) {
        Ok(_) => panic!("invalid workbook accepted"),
        Err(error) => error,
    }
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn text_cell(row: u32, column: usize, value: &str) -> String {
    let letter = char::from(b'A' + u8::try_from(column).unwrap_or_default());
    format!(
        "<c r=\"{letter}{row}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
        xml_text(value)
    )
}

fn valid_row<'a>(account: &'a str, seat: &'a str) -> [&'a str; 9] {
    [
        "示例大学",
        "Example University",
        "CHN",
        account,
        " secret & literal #N/A ",
        seat,
        "星河(测试),队",
        "Star River",
        "participant",
    ]
}

fn row_xml(row: u32, values: &[&str; 9]) -> String {
    let content: String = values
        .iter()
        .enumerate()
        .map(|(c, v)| text_cell(row, c, v))
        .collect();
    format!("<row r=\"{row}\">{content}</row>")
}

fn workbook_with_sheet(content: &str) -> Vec<u8> {
    let sheet = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{content}</sheetData></worksheet>"
    );
    replace_entry("xl/worksheets/sheet1.xml", sheet.as_bytes())
}

fn replace_entry(name: &str, replacement: &[u8]) -> Vec<u8> {
    let mut archive = checked(ZipArchive::new(Cursor::new(EXAMPLE)));
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = checked(archive.by_index(index));
        checked(writer.start_file(entry.name(), SimpleFileOptions::default()));
        if entry.name() == name {
            checked(writer.write_all(replacement));
        } else {
            checked(std::io::copy(&mut entry, &mut writer));
        }
    }
    checked(writer.finish()).into_inner()
}

fn workbook_with_rows(rows: &[&str]) -> Vec<u8> {
    workbook_with_sheet(&format!("{}{}", row_xml(1, &HEADERS), rows.join("")))
}

#[test]
fn real_excel_example_preserves_text_and_merges_schools() {
    let roster = checked(parse_xlsx(EXAMPLE));
    assert_eq!(roster.teams().len(), 3);
    assert_eq!(roster.organizations().len(), 2);
    let university = &roster.organizations()[0];
    assert_eq!(university.key(), "示例大学");
    assert_eq!(university.name_en(), "Example University");
    assert_eq!(university.country(), "CHN");
    assert_eq!(university.rows(), [2, 3]);
    assert_eq!(roster.teams()[0].organization_key(), university.key());
    assert_eq!(roster.teams()[0].row(), 2);
    assert_eq!(roster.teams()[0].account(), "team-001");
    assert_eq!(roster.teams()[0].password(), "example-password-001");
    assert_eq!(roster.teams()[0].seat(), "A-01");
    assert_eq!(roster.teams()[0].name_zh(), "星河");
    assert_eq!(roster.teams()[0].name_en(), "Star River");
    assert_eq!(roster.teams()[0].category(), "participant");
    assert!(!format!("{roster:?}").contains("example-password"));
}

#[test]
fn inline_strings_column_reordering_and_secret_whitespace_are_preserved() {
    let mut headers = HEADERS;
    let mut values = valid_row("0001", "0007");
    headers.swap(0, 4);
    values.swap(0, 4);
    let data = workbook_with_sheet(&format!("{}{}", row_xml(1, &headers), row_xml(2, &values)));
    let roster = checked(parse_xlsx(&data));
    assert_eq!(roster.teams()[0].account(), "0001");
    assert_eq!(roster.teams()[0].seat(), "0007");
    assert_eq!(roster.teams()[0].password(), " secret & literal #N/A ");
    assert_eq!(roster.teams()[0].name_zh(), "星河(测试),队");
    assert!(!format!("{roster:?}").contains("secret & literal"));
}

#[test]
fn blank_english_names_can_be_filled_by_later_rows() {
    let mut first = valid_row("one", "1");
    first[1] = "";
    first[2] = "";
    let data = workbook_with_rows(&[&row_xml(2, &first), &row_xml(4, &valid_row("two", "2"))]);
    let roster = checked(parse_xlsx(&data));
    assert_eq!(roster.organizations()[0].name_en(), "Example University");
    assert_eq!(roster.organizations()[0].rows(), [2, 4]);
}

#[test]
fn english_only_organizations_and_teams_are_valid() {
    let mut values = valid_row("one", "1");
    values[0] = "";
    values[6] = "";
    let roster = checked(parse_xlsx(&workbook_with_rows(&[&row_xml(2, &values)])));
    assert_eq!(roster.organizations()[0].key(), "Example University");
    assert_eq!(roster.teams()[0].organization_key(), "Example University");
}

#[test]
fn missing_and_duplicate_headers_are_rejected_without_echoing_cells() {
    let mut headers = HEADERS;
    headers[4] = "password-canary";
    let error = rejected(&workbook_with_sheet(&row_xml(1, &headers)));
    assert_eq!(error, InputError::cell(1, 5, Kind::Header));
    assert!(!format!("{error} {error:?}").contains("password-canary"));
    headers[4] = "account";
    assert_eq!(
        rejected(&workbook_with_sheet(&row_xml(1, &headers))).kind,
        Kind::Header
    );
    headers[4] = "";
    assert_eq!(
        rejected(&workbook_with_sheet(&row_xml(1, &headers))).kind,
        Kind::MissingColumn("password")
    );
}

#[test]
fn empty_template_requires_a_team() {
    let template = include_bytes!("../../examples/template.xlsx");
    assert_eq!(rejected(template).kind, Kind::EmptyRoster);
}

#[test]
fn formulas_cached_values_and_real_error_cells_are_rejected() {
    let cells = [
        (
            "<c r=\"E2\" t=\"str\"><f>password-canary</f><v>cached-password</v></c>",
            Kind::Formula,
        ),
        (
            "<c r=\"E2\"><f t=\"shared\" si=\"9\"/><v>0</v></c>",
            Kind::Formula,
        ),
        ("<c r=\"E2\" t=\"e\"><v>#N/A</v></c>", Kind::CellError),
        ("<c r=\"E2\"><v>12345</v></c>", Kind::TextRequired),
        ("<c r=\"E2\" t=\"b\"><v>1</v></c>", Kind::TextRequired),
    ];
    for (cell, kind) in cells {
        let data = workbook_with_rows(&[&format!("<row r=\"2\">{cell}</row>")]);
        let error = rejected(&data);
        assert_eq!(error, InputError::cell(2, 5, kind));
        let diagnostics = format!("{error} {error:?}");
        assert!(!diagnostics.contains("password-canary"));
        assert!(!diagnostics.contains("cached-password"));
        assert!(!diagnostics.contains("12345"));
    }
}

#[test]
fn typed_field_and_duplicate_diagnostics_point_to_the_input_column() {
    let cases = [
        (0, "", Kind::Required),
        (3, "team+one", Kind::InvalidIdentifier),
        (3, ".one", Kind::InvalidIdentifier),
        (3, "-one", Kind::InvalidIdentifier),
        (8, "group.", Kind::InvalidIdentifier),
        (2, "cn", Kind::InvalidCountry),
        (4, "", Kind::Required),
        (4, "line\nbreak-canary", Kind::ControlCharacter),
        (5, "", Kind::Required),
    ];
    for (column, value, kind) in cases {
        let mut values = valid_row("one", "1");
        if column == 0 {
            values[1] = "";
        }
        values[column] = value;
        let error = rejected(&workbook_with_rows(&[&row_xml(2, &values)]));
        assert_eq!(
            error,
            InputError::cell(2, u32::try_from(column + 1).unwrap_or_default(), kind)
        );
        assert!(!format!("{error} {error:?}").contains("canary"));
    }
    for (second, column, kind) in [
        (valid_row("one", "2"), 4, Kind::DuplicateAccount(2)),
        (valid_row("two", "1"), 6, Kind::DuplicateSeat(2)),
    ] {
        let data = workbook_with_rows(&[&row_xml(2, &valid_row("one", "1")), &row_xml(5, &second)]);
        assert_eq!(rejected(&data), InputError::cell(5, column, kind));
    }
}

#[test]
fn names_and_country_must_agree_within_one_school() {
    for (column, value) in [(1, "Different University"), (2, "USA")] {
        let mut second = valid_row("two", "2");
        second[column] = value;
        let data = workbook_with_rows(&[&row_xml(2, &valid_row("one", "1")), &row_xml(3, &second)]);
        assert_eq!(
            rejected(&data),
            InputError::cell(
                3,
                u32::try_from(column + 1).unwrap_or_default(),
                Kind::OrganizationConflict(2)
            )
        );
    }
}

#[test]
fn limits_reject_long_fields_and_sparse_cells_without_allocating_sheet_dimensions() {
    let long_password = "secret-canary".repeat(50);
    let mut values = valid_row("one", "1");
    values[4] = &long_password;
    let error = rejected(&workbook_with_rows(&[&row_xml(2, &values)]));
    assert_eq!(error, InputError::cell(2, 5, Kind::FieldTooLong(512)));
    assert!(!format!("{error:?}").contains("secret-canary"));
    for (reference, kind) in [
        ("A1048576", Kind::TooManyRows),
        ("XFD2", Kind::UnexpectedColumn),
    ] {
        let cell = format!("<row><c r=\"{reference}\" t=\"inlineStr\"><is><t>x</t></is></c></row>");
        assert_eq!(rejected(&workbook_with_rows(&[&cell])).kind, kind);
    }
    let repeated = format!(
        "<row r=\"2\">{}{}</row>",
        text_cell(2, 0, "first"),
        text_cell(2, 0, "second")
    );
    assert_eq!(
        rejected(&workbook_with_rows(&[&repeated])).kind,
        Kind::RepeatedCell
    );
}

#[test]
fn invalid_and_oversized_archives_have_content_free_errors() {
    assert_eq!(
        rejected(b"not a workbook password-canary").kind,
        Kind::Archive
    );
    assert_eq!(
        rejected(&vec![0; MAX_WORKBOOK_BYTES + 1]).kind,
        Kind::WorkbookTooLarge
    );
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    checked(writer.start_file(
        "oversized",
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
    ));
    checked(std::io::copy(
        &mut std::io::repeat(0).take(MAX_EXPANDED_BYTES + 1),
        &mut writer,
    ));
    assert_eq!(
        rejected(&checked(writer.finish()).into_inner()).kind,
        Kind::ExpandedWorkbookTooLarge
    );
}

#[test]
fn rejects_missing_teams_sheet_and_malformed_xml_without_content_leaks() {
    let missing = replace_entry("xl/workbook.xml", b"<workbook xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"Other\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>");
    assert_eq!(rejected(&missing).kind, Kind::MissingWorksheet);
    let malformed = replace_entry(
        "xl/worksheets/sheet1.xml",
        b"<worksheet><sheetData><row><c t=\"inlineStr\"><is><t>password-canary</is></row>",
    );
    let error = rejected(&malformed);
    assert!(!format!("{error} {error:?}").contains("password-canary"));
}

#[test]
fn ten_thousand_rows_fit_and_preserve_original_row_locations() {
    let mut body = String::new();
    for row in 2..=MAX_DATA_ROWS + 1 {
        checked(write!(
            body,
            "{}",
            row_xml(row, &valid_row(&format!("team-{row}"), &row.to_string()))
        ));
    }
    let roster = checked(parse_xlsx(&workbook_with_rows(&[&body])));
    assert_eq!(roster.teams().len(), 10_000);
    assert_eq!(roster.organizations().len(), 1);
    assert_eq!(roster.teams()[9_999].row(), 10_001);
}
