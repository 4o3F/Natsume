use std::{
    fs,
    process::{Command, Output},
};

use image::{DynamicImage, ImageFormat};
use tempfile::TempDir;

fn checked<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("test setup failed: {error:?}"))
}

fn run(directory: &TempDir) -> Output {
    checked(
        Command::new(env!("CARGO_BIN_EXE_natsume-check-logos"))
            .arg(directory.path().join("roster.xlsx"))
            .arg("--logos")
            .arg(directory.path().join("logos"))
            .output(),
    )
}

fn report(output: &Output) -> String {
    assert!(output.stdout.is_empty(), "diagnostics belong on stderr");
    let report = checked(String::from_utf8(output.stderr.clone()));
    assert!(
        !report.contains('\u{1b}'),
        "logs must not contain ANSI escapes"
    );
    assert!(!report.contains("example-password"));
    assert!(!report.contains("password-canary"));
    report
}

#[test]
fn executable_reports_missing_ambiguous_invalid_and_fixed_logos_without_mutations() {
    let directory = checked(TempDir::new());
    let workbook = directory.path().join("roster.xlsx");
    let input = include_bytes!("../examples/roster.xlsx");
    checked(fs::write(&workbook, input));
    let logos = directory.path().join("logos");
    checked(fs::create_dir(&logos));
    let missing = run(&directory);
    assert_eq!(missing.status.code(), Some(1));
    let missing_report = report(&missing);
    assert!(missing_report.contains("schools=2 matched=0 missing=2 ambiguous=0 invalid=0"));
    assert!(missing_report.lines().any(|line| line.contains("WARN")
        && line.contains("School logo is missing")
        && line.contains("school=\"示例大学\"")
        && line.contains("rows=[2, 3]")
        && line.contains("expected_stems=")));

    let university = logos.join("示例大学.webp");
    let college = logos.join("示例学院.webp");
    let extra = logos.join("Example University.png");
    checked(DynamicImage::new_rgb8(8, 8).save_with_format(&university, ImageFormat::WebP));
    checked(DynamicImage::new_rgb8(8, 8).save_with_format(&extra, ImageFormat::Png));
    checked(fs::write(&college, b"broken image"));
    let invalid = run(&directory);
    assert_eq!(invalid.status.code(), Some(1));
    let invalid_report = report(&invalid);
    assert!(invalid_report.contains("schools=2 matched=0 missing=0 ambiguous=1 invalid=1"));
    assert!(invalid_report.lines().any(|line| line.contains("WARN")
        && line.contains("School logo is ambiguous")
        && line.contains("paths=")));
    assert!(invalid_report.lines().any(|line| line.contains("ERROR")
        && line.contains("School logo is invalid")
        && line.contains("error=")));

    checked(fs::remove_file(extra));
    checked(fs::write(&university, br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#336699"/></svg>"##));
    checked(DynamicImage::new_rgb8(8, 8).save_with_format(&college, ImageFormat::Png));
    let before = checked(fs::read(&university));
    let fixed = run(&directory);
    assert_eq!(fixed.status.code(), Some(0));
    let fixed_report = report(&fixed);
    assert_eq!(fixed_report.lines().count(), 1);
    assert!(fixed_report.contains("INFO"));
    assert!(fixed_report.contains("Logo check completed"));
    assert!(fixed_report.contains("schools=2 matched=2 missing=0 ambiguous=0 invalid=0"));
    assert_eq!(checked(fs::read(&workbook)), input);
    assert_eq!(checked(fs::read(&university)), before);
    assert_eq!(checked(fs::read_dir(&logos)).count(), 2);
}

#[test]
fn executable_uses_exit_two_for_bad_input_and_help_needs_no_server() {
    let directory = checked(TempDir::new());
    let missing = run(&directory);
    assert_eq!(missing.status.code(), Some(2));
    let missing_report = report(&missing);
    assert!(missing_report.contains("ERROR"));
    assert!(missing_report.contains("Logo check failed"));
    assert!(missing_report.contains("cannot read workbook"));
    checked(fs::write(
        directory.path().join("roster.xlsx"),
        b"not XLSX password-canary",
    ));
    let invalid = run(&directory);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(report(&invalid).contains("ERROR"));
    checked(fs::write(
        directory.path().join("roster.xlsx"),
        include_bytes!("../examples/roster.xlsx"),
    ));
    let missing_directory = run(&directory);
    assert_eq!(missing_directory.status.code(), Some(2));
    assert!(report(&missing_directory).contains("cannot read logo directory"));
    let help = checked(
        Command::new(env!("CARGO_BIN_EXE_natsume-check-logos"))
            .arg("--help")
            .output(),
    );
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--logos"));
}
