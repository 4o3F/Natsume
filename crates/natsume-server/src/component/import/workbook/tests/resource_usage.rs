//! Opt-in, process-isolated measurements; not a production capacity assertion.
use std::{fs, path::Path, time::Instant};

use super::*;

#[test]
#[ignore = "writes synthetic resource fixtures to NATSUME_RESOURCE_FIXTURES"]
fn generate_workbook_resource_fixtures() {
    let directory = checked(std::env::var("NATSUME_RESOURCE_FIXTURES"));
    let directory = Path::new(&directory);
    checked(fs::create_dir_all(directory));
    for (name, count, field_length, unique) in [
        ("nominal", 600, 32, false),
        ("maximum", 10_000, 1024, false),
        ("maximum-unique", 10_000, 1024, true),
    ] {
        let password = "p".repeat(512);
        let category = "c".repeat(36);
        let mut sheet = row_xml(1, &HEADERS);
        for index in 0..count {
            let name_field = if unique {
                format!("{index:05}{}", "N".repeat(field_length - 5))
            } else {
                "N".repeat(field_length)
            };
            let account = format!("team-{index:05}{}", "a".repeat(26));
            let seat = format!("seat-{index:05}{}", "s".repeat(54));
            sheet.push_str(&row_xml(
                index + 2,
                &[
                    &name_field,
                    &name_field,
                    "CHN",
                    &account,
                    &password,
                    &seat,
                    &name_field,
                    &name_field,
                    &category,
                ],
            ));
        }
        checked(fs::write(
            directory.join(format!("{name}.xlsx")),
            workbook_with_sheet(&sheet),
        ));
    }
    let header_bytes = HEADERS.iter().map(|value| value.len()).sum::<usize>();
    checked(fs::write(
        directory.join("shared-amplification.xlsx"),
        shared_string_workbook(header_bytes + 10_000 * 9 * 4096),
    ));
}

fn shared_string_workbook(mut cell_bytes: usize) -> Vec<u8> {
    let mut sheet = row_xml(1, &HEADERS);
    cell_bytes -= HEADERS.iter().map(|value| value.len()).sum::<usize>();
    let mut row = 2;
    while cell_bytes > 0 {
        checked(write!(sheet, "<row r=\"{row}\">"));
        for (column, letter) in (b'A'..=b'I').enumerate() {
            if cell_bytes == 0 {
                break;
            }
            let length = cell_bytes.min(4096);
            if length == 4096 {
                checked(write!(
                    sheet,
                    "<c r=\"{}{row}\" t=\"s\"><v>0</v></c>",
                    char::from(letter)
                ));
            } else {
                sheet.push_str(&text_cell(row, column, &"x".repeat(length)));
            }
            cell_bytes -= length;
        }
        sheet.push_str("</row>");
        row += 1;
    }
    let strings = format!(
        "<sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><si><t>{}</t></si></sst>",
        "x".repeat(4096)
    );
    let base = workbook_with_sheet(&sheet);
    let mut archive = checked(ZipArchive::new(Cursor::new(base)));
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = checked(archive.by_index(index));
        let name = checked(entry.name());
        checked(writer.start_file(&name, SimpleFileOptions::default()));
        if name == "xl/sharedStrings.xml" {
            checked(writer.write_all(strings.as_bytes()));
        } else {
            checked(std::io::copy(&mut entry, &mut writer));
        }
    }
    checked(writer.finish()).into_inner()
}

#[test]
fn shared_string_references_enforce_the_exact_retained_text_boundary() {
    let limit = checked(usize::try_from(MAX_CELL_TEXT_BYTES));
    for size in [limit - 1, limit, limit + 1] {
        let error = rejected(&shared_string_workbook(size));
        assert_eq!(
            error.kind,
            if size > limit {
                Kind::CellDataTooLarge
            } else {
                Kind::FieldTooLong(1024)
            }
        );
        assert!(!error.to_string().contains(&"x".repeat(64)));
    }
}

#[test]
#[ignore = "loads NATSUME_RESOURCE_WORKBOOK; run each input in a separate process"]
fn measure_workbook_resource_usage() {
    let path = checked(std::env::var("NATSUME_RESOURCE_WORKBOOK"));
    let bytes = checked(fs::read(&path));
    let started = Instant::now();
    let result = parse_xlsx(&bytes);
    eprintln!(
        "workbook_bytes={}, parse_elapsed_ms={}",
        bytes.len(),
        started.elapsed().as_millis()
    );
    match &result {
        Ok(roster) => eprintln!(
            "teams={}, organizations={}",
            roster.teams().len(),
            roster.organizations().len()
        ),
        Err(error) => eprintln!("rejection={error}"),
    }
    match Path::new(&path).file_name().and_then(|name| name.to_str()) {
        Some("nominal.xlsx") => {
            let roster = checked(result);
            assert_eq!(
                (roster.teams().len(), roster.organizations().len()),
                (600, 1)
            );
        }
        Some("maximum.xlsx") => {
            let roster = checked(result);
            assert_eq!(
                (roster.teams().len(), roster.organizations().len()),
                (10_000, 1)
            );
        }
        Some("maximum-unique.xlsx") => {
            let roster = checked(result);
            assert_eq!(
                (roster.teams().len(), roster.organizations().len()),
                (10_000, 10_000)
            );
        }
        Some("shared-amplification.xlsx") => {
            assert_eq!(
                result.err().map(|error| error.kind),
                Some(Kind::CellDataTooLarge)
            );
        }
        _ => panic!("use only the generated synthetic resource fixtures"),
    }
    for line in checked(fs::read_to_string("/proc/self/status"))
        .lines()
        .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
    {
        eprintln!("{line}");
    }
}
