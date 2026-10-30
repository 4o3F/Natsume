#![cfg(windows)]
use snafu::{ResultExt as _, Snafu};
use std::process::Command;

#[derive(Debug, Snafu)]
enum Error {
    Io { source: std::io::Error },
}
type Result<T = ()> = std::result::Result<T, Error>;

#[test]
fn help_lists_only_initialization_and_offline_diagnosis() -> Result {
    let output = Command::new(env!("CARGO_BIN_EXE_natsume-photo-client"))
        .arg("--help")
        .output()
        .context(IoSnafu)?;
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("initialize"));
    assert!(text.contains("doctor"));
    for removed in ["check-roster", "create-workspace", "inspect-workspace"] {
        assert!(!text.contains(removed));
        assert!(
            !Command::new(env!("CARGO_BIN_EXE_natsume-photo-client"))
                .arg(removed)
                .output()
                .context(IoSnafu)?
                .status
                .success()
        );
    }
    Ok(())
}

#[test]
fn diagnosis_never_creates_missing_workspace() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let path = directory.path().join("missing");
    let output = Command::new(env!("CARGO_BIN_EXE_natsume-photo-client"))
        .arg("doctor")
        .arg(&path)
        .output()
        .context(IoSnafu)?;
    assert!(!output.status.success());
    assert!(!path.exists());
    Ok(())
}
