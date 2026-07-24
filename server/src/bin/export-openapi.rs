use std::{env, fs, path::PathBuf, process::ExitCode};

use natsume_error_code::redact_report;

fn run() -> Result<(), &'static str> {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: export-openapi <output.json>")?;
    let document = natsume_server::openapi::openapi()
        .to_pretty_json()
        .map_err(|_| "OPENAPI_EXPORT_SERIALIZE_FAILED")?;
    fs::write(output, format!("{document}\n")).map_err(|_| "OPENAPI_EXPORT_WRITE_FAILED")
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", redact_report(error));
            ExitCode::FAILURE
        }
    }
}
