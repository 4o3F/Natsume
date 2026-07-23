use std::{env, fs, path::PathBuf};

use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("usage: export-openapi <output.json>"))]
    MissingOutput,

    #[snafu(display("failed to write OpenAPI snapshot to {}", path.display()))]
    WriteSnapshot {
        path: PathBuf,
        source: std::io::Error,
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
    fs::write(&output, document).context(WriteSnapshotSnafu { path: output })?;
    Ok(())
}
