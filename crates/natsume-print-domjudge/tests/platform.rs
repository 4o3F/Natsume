#![cfg(not(target_os = "linux"))]

use std::process::Command;

#[test]
fn wrapper_reports_unsupported_platform_on_stdout() {
    let output = Command::new(env!("CARGO_BIN_EXE_natsume-print-domjudge"))
        .output()
        .unwrap_or_else(|error| panic!("run DOMjudge print wrapper: {error}"));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("requires Linux"));
    assert!(output.stderr.is_empty());
}
