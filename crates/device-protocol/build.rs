use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("vendored protoc is unavailable for this build target"))]
    ProtocUnavailable { source: protoc_bin_vendored::Error },

    #[snafu(display("failed to generate Rust code from proto/device_control.proto"))]
    GenerateProtocol { source: std::io::Error },
}

#[snafu::report]
fn main() -> Result<(), Error> {
    let protoc = protoc_bin_vendored::protoc_bin_path().context(ProtocUnavailableSnafu)?;
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config
        .compile_protos(&["proto/device_control.proto"], &["proto"])
        .context(GenerateProtocolSnafu)?;
    println!("cargo:rerun-if-changed=proto/device_control.proto");
    Ok(())
}
