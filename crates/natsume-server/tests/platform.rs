#![cfg(not(target_os = "linux"))]

use std::process::Command;

#[test]
fn server_rejects_unsupported_platforms() {
    let output = Command::new(env!("CARGO_BIN_EXE_natsume-server"))
        .output()
        .unwrap_or_else(|error| panic!("run Server: {error}"));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("requires Linux"));
}

#[test]
fn typescript_exporter_rejects_unsupported_platforms() {
    let output = Command::new(env!("CARGO_BIN_EXE_export-typescript"))
        .output()
        .unwrap_or_else(|error| panic!("run TypeScript exporter: {error}"));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("requires Linux"));
}
