use std::{
    fs::File,
    io::{self, Read},
    path::PathBuf,
    process::ExitCode,
};

use clap::Parser;
use natsume_roster::{LogoDirectory, LogoMatch, MAX_WORKBOOK_BYTES, parse_xlsx, validate_logo};

#[derive(Parser)]
#[command(about = "Check school logos against a Natsume Teams XLSX workbook (offline, read-only)")]
struct Arguments {
    /// Workbook using the fixed Teams template.
    workbook: PathBuf,
    /// Directory containing images named after complete school names.
    #[arg(long)]
    logos: PathBuf,
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_writer(io::stderr)
        .init();
    match run(&arguments) {
        Ok(code) => code,
        Err(error) => {
            tracing::error!(%error, "Logo check failed");
            ExitCode::from(2)
        }
    }
}

#[allow(
    clippy::unnecessary_debug_formatting,
    reason = "Keep paths quoted and escape control characters in terminal reports."
)]
fn run(arguments: &Arguments) -> Result<ExitCode, String> {
    let mut bytes = Vec::new();
    File::open(&arguments.workbook)
        .and_then(|file| {
            file.take(MAX_WORKBOOK_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| format!("cannot read workbook {:?}: {error}", arguments.workbook))?;
    let roster = parse_xlsx(&bytes).map_err(|error| error.to_string())?;
    let directory = LogoDirectory::open(&arguments.logos)
        .map_err(|error| format!("cannot read logo directory {:?}: {error}", arguments.logos))?;
    let mut matched = 0;
    let mut missing = 0;
    let mut ambiguous = 0;
    let mut invalid = 0;
    for organization in roster.organizations() {
        match directory.resolve(organization.name_zh(), organization.name_en()) {
            LogoMatch::Missing => {
                missing += 1;
                let expected: Vec<_> = [organization.name_zh(), organization.name_en()]
                    .into_iter()
                    .filter(|name| !name.is_empty())
                    .collect();
                tracing::warn!(
                    school = organization.key(),
                    rows = ?organization.rows(),
                    expected_stems = ?expected,
                    "School logo is missing"
                );
            }
            LogoMatch::Ambiguous(paths) => {
                ambiguous += 1;
                tracing::warn!(
                    school = organization.key(),
                    rows = ?organization.rows(),
                    ?paths,
                    "School logo is ambiguous"
                );
            }
            LogoMatch::Unique(path) => match validate_logo(&path) {
                Ok(()) => {
                    matched += 1;
                }
                Err(error) => {
                    invalid += 1;
                    tracing::error!(
                        school = organization.key(),
                        rows = ?organization.rows(),
                        ?path,
                        %error,
                        "School logo is invalid"
                    );
                }
            },
        }
    }
    tracing::info!(
        schools = roster.organizations().len(),
        matched,
        missing,
        ambiguous,
        invalid,
        "Logo check completed"
    );
    Ok(ExitCode::from(u8::from(missing + ambiguous + invalid > 0)))
}
