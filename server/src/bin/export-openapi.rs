use std::{
    ffi::OsString,
    fs,
    io::{self, Write as _},
    path::PathBuf,
    process::ExitCode,
};

use snafu::Snafu;

const EXPORT_FAILURE_ID: &str = "NATSUME_OPENAPI_EXPORT_FAILED";

fn main() -> ExitCode {
    if export(std::env::args_os().skip(1)).is_err() {
        let _write_result = writeln!(io::stderr().lock(), "{EXPORT_FAILURE_ID}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn export<I>(args: I) -> Result<(), ExportError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let Some(output_path) = args.next() else {
        return Err(ExportError::InvalidArguments);
    };
    if args.next().is_some() {
        return Err(ExportError::InvalidArguments);
    }

    let document = natsume_server::openapi::document();
    let mut encoded = serde_json::to_string_pretty(&document)
        .unwrap_or_else(|_| panic!("OpenAPI document serialization invariant failed"));
    encoded.push('\n');
    fs::write(PathBuf::from(output_path), encoded).map_err(|_| ExportError::WriteFailed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
enum ExportError {
    #[snafu(display("the OpenAPI exporter arguments are invalid"))]
    InvalidArguments,
    #[snafu(display("the OpenAPI document could not be written"))]
    WriteFailed,
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, fs, path::PathBuf};

    use snafu::Snafu;
    use uuid::Uuid;

    use super::{EXPORT_FAILURE_ID, ExportError, export};

    #[test]
    fn consecutive_exports_are_stable_with_one_trailing_newline() -> Result<(), TestFailure> {
        let fixture = ExportFixture::new();
        export([fixture.first.clone().into_os_string()]).map_err(|_| TestFailure::ExportFailed)?;
        export([fixture.second.clone().into_os_string()]).map_err(|_| TestFailure::ExportFailed)?;

        let first = fs::read(&fixture.first).map_err(|_| TestFailure::ReadFailed)?;
        let second = fs::read(&fixture.second).map_err(|_| TestFailure::ReadFailed)?;
        if first != second
            || !first.ends_with(b"\n")
            || first.ends_with(b"\n\n")
            || serde_json::from_slice::<serde_json::Value>(&first).is_err()
        {
            return Err(TestFailure::OutputWasNotStablePrettyJson);
        }
        Ok(())
    }

    #[test]
    fn argument_failures_are_closed_and_redacted() -> Result<(), TestFailure> {
        let zero = export(Vec::<OsString>::new())
            .err()
            .ok_or(TestFailure::InvalidArgumentsWereAccepted)?;
        let path_canary = "openapi-output-path-canary";
        let two = export([
            OsString::from(path_canary),
            OsString::from("second-argument-canary"),
        ])
        .err()
        .ok_or(TestFailure::InvalidArgumentsWereAccepted)?;
        if zero != ExportError::InvalidArguments || two != zero {
            return Err(TestFailure::ArgumentFailureWasNotTyped);
        }
        for encoded in [zero.to_string(), format!("{zero:?}")] {
            if encoded.contains(path_canary)
                || encoded.contains("second-argument-canary")
                || encoded.contains("serde")
            {
                return Err(TestFailure::ArgumentFailureWasNotRedacted);
            }
        }
        if EXPORT_FAILURE_ID != "NATSUME_OPENAPI_EXPORT_FAILED" {
            return Err(TestFailure::FailureIdentifierChanged);
        }
        Ok(())
    }

    struct ExportFixture {
        first: PathBuf,
        second: PathBuf,
    }

    impl ExportFixture {
        fn new() -> Self {
            let suffix = Uuid::now_v7();
            Self {
                first: std::env::temp_dir().join(format!("natsume-openapi-first-{suffix}.json")),
                second: std::env::temp_dir().join(format!("natsume-openapi-second-{suffix}.json")),
            }
        }
    }

    impl Drop for ExportFixture {
        fn drop(&mut self) {
            let _first_result = fs::remove_file(&self.first);
            let _second_result = fs::remove_file(&self.second);
        }
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the OpenAPI export failed"))]
        ExportFailed,
        #[snafu(display("the OpenAPI export could not be read"))]
        ReadFailed,
        #[snafu(display("the OpenAPI output was not stable pretty JSON"))]
        OutputWasNotStablePrettyJson,
        #[snafu(display("invalid exporter arguments were accepted"))]
        InvalidArgumentsWereAccepted,
        #[snafu(display("the exporter argument failure was not typed"))]
        ArgumentFailureWasNotTyped,
        #[snafu(display("the exporter argument failure was not redacted"))]
        ArgumentFailureWasNotRedacted,
        #[snafu(display("the exporter failure identifier changed"))]
        FailureIdentifierChanged,
    }
}
