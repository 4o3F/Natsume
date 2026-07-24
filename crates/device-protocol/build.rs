use std::{env, path::PathBuf};

use snafu::{OptionExt, ResultExt, Snafu};

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("vendored protoc is unavailable for this build target"))]
    ProtocUnavailable { source: protoc_bin_vendored::Error },

    #[snafu(display("Cargo did not provide OUT_DIR to the protocol build"))]
    MissingOutDir,

    #[snafu(display(
        "failed to generate Rust code and descriptor from proto/device_control.proto"
    ))]
    GenerateProtocol { source: std::io::Error },
}

#[snafu::report]
fn main() -> Result<(), Error> {
    let protoc = protoc_bin_vendored::protoc_bin_path().context(ProtocUnavailableSnafu)?;
    let out_dir = env::var_os("OUT_DIR").context(MissingOutDirSnafu)?;
    let descriptor_path = PathBuf::from(out_dir).join("device_control.pb");
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config.file_descriptor_set_path(descriptor_path);
    config
        .compile_protos(&["proto/device_control.proto"], &["proto"])
        .context(GenerateProtocolSnafu)?;
    println!("cargo:rerun-if-changed=proto/device_control.proto");
    Ok(())
}
