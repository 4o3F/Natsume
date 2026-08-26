use std::{env, io, path::PathBuf};

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let out_dir = env::var_os("OUT_DIR").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Cargo did not provide OUT_DIR to the protocol build",
        )
    })?;
    let descriptor_path = PathBuf::from(out_dir).join("device_control.pb");
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config.file_descriptor_set_path(descriptor_path);
    config.skip_debug([".natsume.device.control.SecretBytes"]);
    config.compile_protos(&PROTOS, &["proto"])?;
    for proto in PROTOS {
        println!("cargo:rerun-if-changed={proto}");
    }
    Ok(())
}
