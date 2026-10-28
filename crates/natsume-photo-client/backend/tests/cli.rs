#![cfg(windows)]
use snafu::{ResultExt as _, Snafu};
use std::process::Command;

#[derive(Debug, Snafu)]
enum Error {
    Io {
        source: std::io::Error,
    },
    Roster {
        source: natsume_photo_client::roster::Error,
    },
    Workspace {
        source: natsume_photo_client::workspace::Error,
    },
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

#[tokio::test]
async fn diagnosis_rejects_unauthorized_fixture_without_changing_database_or_exposing_contents()
-> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let root = directory.path().join("workspace");
    let roster =
        natsume_photo_client::roster::Roster::parse(include_bytes!("fixtures/roster.json"))
            .context(RosterSnafu)?;
    drop(
        natsume_photo_client::workspace::Workspace::create(&root, &roster)
            .await
            .context(WorkspaceSnafu)?,
    );
    std::fs::write(
        root.join("authorization.json"),
        br#"{"secret-marker-123":"private-value-456"}"#,
    )
    .context(IoSnafu)?;
    let before = std::fs::read(root.join("workspace.sqlite3")).context(IoSnafu)?;
    let output = Command::new(env!("CARGO_BIN_EXE_natsume-photo-client"))
        .arg("doctor")
        .arg(&root)
        .output()
        .context(IoSnafu)?;
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read(root.join("workspace.sqlite3")).context(IoSnafu)?,
        before
    );
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains("secret-marker-123"));
        assert!(!text.contains("private-value-456"));
    }
    Ok(())
}
