use std::{env, path::PathBuf};

use snafu::{OptionExt, ResultExt, Snafu};

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("vendored protoc is unavailable for this build target"))]
    ProtocUnavailable { source: protoc_bin_vendored::Error },

    #[snafu(display("Cargo did not provide OUT_DIR to the protocol build"))]
    MissingOutDir,

    #[snafu(display("failed to generate split Device control protocol"))]
    GenerateProtocol { source: std::io::Error },
}

const PROTOS: [&str; 8] = [
    "proto/device_control.proto",
    "proto/device_control_common.proto",
    "proto/device_control_handshake.proto",
    "proto/device_control_binding.proto",
    "proto/device_control_gateway.proto",
    "proto/device_control_runtime.proto",
    "proto/device_control_session.proto",
    "proto/device_control_state.proto",
];

#[snafu::report]
fn main() -> Result<(), Error> {
    let protoc = protoc_bin_vendored::protoc_bin_path().context(ProtocUnavailableSnafu)?;
    let out_dir = env::var_os("OUT_DIR").context(MissingOutDirSnafu)?;
    let descriptor_path = PathBuf::from(out_dir).join("device_control.pb");
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config.file_descriptor_set_path(descriptor_path);
    config.skip_debug([".natsume.device.control.SecretBytes"]);
    config
        .compile_protos(&PROTOS, &["proto"])
        .context(GenerateProtocolSnafu)?;
    for proto in PROTOS {
        println!("cargo:rerun-if-changed={proto}");
    }
    Ok(())
}
