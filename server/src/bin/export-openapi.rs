use std::{env, fs, io, path::PathBuf};

use natsume_error_code::Redacted;
use snafu::Snafu;

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("usage: export-openapi <output.json>"))]
    MissingOutput,

    #[snafu(display("failed to write OpenAPI snapshot"))]
    WriteSnapshot {
        path: Redacted<PathBuf>,
        kind: io::ErrorKind,
    },
}

#[snafu::report]
fn main() -> Result<(), Error> {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or(Error::MissingOutput)?;
    // Blueprint placeholder: the implementation exports utoipa-derived Rust API types.
    let document = include_str!("../../../web/openapi/natsume.openapi.json");
    fs::write(&output, document).map_err(|error| Error::WriteSnapshot {
        path: Redacted::new(output),
        kind: error.kind(),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{error::Error as _, io, path::PathBuf};

    use natsume_error_code::Redacted;

    use super::Error;

    #[test]
    fn write_error_debug_and_display_do_not_leak_the_output_path() {
        let error = Error::WriteSnapshot {
            path: Redacted::new(PathBuf::from("/home/operator/private/openapi.json")),
            kind: io::ErrorKind::PermissionDenied,
        };
        let display = format!("{error}");
        let debug = format!("{error:?}");

        assert_eq!(display, "failed to write OpenAPI snapshot");
        assert!(!display.contains("/home/operator"));
        assert!(!debug.contains("/home/operator"));
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("PermissionDenied"));
        assert!(error.source().is_none());
    }
}
