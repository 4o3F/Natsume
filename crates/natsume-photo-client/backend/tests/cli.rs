use std::{
    path::Path,
    process::{Command, Output},
};

use snafu::{ResultExt as _, Snafu};

#[derive(Debug, Snafu)]
enum Error {
    Io { source: std::io::Error },
}
type Result<T = ()> = std::result::Result<T, Error>;

fn check(path: &Path) -> Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_natsume-photo-client"))
        .arg("check-roster")
        .arg(path)
        .output()
        .context(IoSnafu)
}

#[test]
fn valid_roster_reports_count_without_creating_workspace() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let path = directory.path().join("测试 名单.json");
    std::fs::write(&path, include_bytes!("fixtures/roster.json")).context(IoSnafu)?;
    let output = check(&path)?;
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("team_count=2"));
    assert_eq!(
        std::fs::read_dir(directory.path())
            .context(IoSnafu)?
            .count(),
        1
    );
    Ok(())
}

#[test]
fn invalid_roster_fails_without_logging_sensitive_source_details() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let path = directory.path().join("roster.json");
    // A serde error could include an unknown field name; the CLI must not print it.
    std::fs::write(&path, br#"[{"secret-marker-123":"private-value-456"}]"#).context(IoSnafu)?;
    let output = check(&path)?;
    assert!(!output.status.success());
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains("secret-marker-123"));
        assert!(!text.contains("private-value-456"));
    }
    assert!(String::from_utf8_lossy(&output.stdout).contains("名单检查失败"));
    Ok(())
}

#[test]
fn missing_file_and_missing_argument_fail() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    assert!(
        !check(&directory.path().join("missing.json"))?
            .status
            .success()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_natsume-photo-client"))
        .arg("check-roster")
        .output()
        .context(IoSnafu)?;
    assert!(!output.status.success());
    Ok(())
}

#[test]
fn help_exits_successfully() -> Result {
    let output = Command::new(env!("CARGO_BIN_EXE_natsume-photo-client"))
        .arg("--help")
        .output()
        .context(IoSnafu)?;
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("check-roster"));
    Ok(())
}
