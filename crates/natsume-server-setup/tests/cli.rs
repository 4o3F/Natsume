#![cfg(target_os = "linux")]

use snafu::Snafu;
use std::{io, os::unix::process::CommandExt as _, process::Command};

#[derive(Debug, Snafu)]
#[snafu(context(false), display("cannot execute setup CLI fixture"))]
struct TestError {
    source: io::Error,
}

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_natsume-server-setup"));
    if rustix::process::geteuid().is_root() {
        command.gid(65534).uid(65534);
    }
    command
}

#[test]
fn non_root_default_wizard_fails_preflight_with_exit_one() -> Result<(), TestError> {
    let output = command().output()?;
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("run setup as root"));
    assert!(stderr.contains("host preflight failed"));
    Ok(())
}

#[test]
fn probe_rejects_non_service_user_with_exit_one() -> Result<(), TestError> {
    let output = command().args(["probe", "deployment"]).output()?;
    assert_eq!(output.status.code(), Some(1));
    Ok(())
}

#[test]
fn unknown_cli_argument_exits_two() -> Result<(), TestError> {
    let output = command().arg("--unknown-option").output()?;
    assert_eq!(output.status.code(), Some(2));
    Ok(())
}
