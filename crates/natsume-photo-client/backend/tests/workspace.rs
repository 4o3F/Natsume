use std::process::Command;

use natsume_photo_client::{
    roster::Roster,
    workspace::{Error as WorkspaceError, Workspace},
};
use snafu::{ResultExt as _, Snafu};

const FIXTURE: &[u8] = include_bytes!("fixtures/roster.json");

#[derive(Debug, Snafu)]
enum Error {
    Io {
        source: std::io::Error,
    },
    Roster {
        source: natsume_photo_client::roster::Error,
    },
    Workspace {
        source: WorkspaceError,
    },
}
type Result<T = ()> = std::result::Result<T, Error>;

#[test]
fn roster_survives_close_reopen_and_source_removal() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let source = directory.path().join("roster.json");
    std::fs::write(&source, FIXTURE).context(IoSnafu)?;
    let roster = Roster::load(&source).context(RosterSnafu)?;
    let root = directory.path().join("工作区 #1% 比赛");
    drop(Workspace::create(&root, &roster).context(WorkspaceSnafu)?);
    std::fs::remove_file(&source).context(IoSnafu)?;
    let mut reopened = Workspace::open(&root).context(WorkspaceSnafu)?;
    let persisted = reopened.roster().context(WorkspaceSnafu)?;
    assert_eq!(persisted.teams().len(), 2);
    for (actual, expected) in persisted.teams().iter().zip(roster.teams()) {
        assert_eq!(actual.account_id, expected.account_id);
        assert_eq!(actual.account, expected.account);
        assert_eq!(actual.organization_id, expected.organization_id);
        assert_eq!(actual.school_name_zh, expected.school_name_zh);
        assert_eq!(actual.school_name_en, expected.school_name_en);
        assert_eq!(actual.team_name_zh, expected.team_name_zh);
        assert_eq!(actual.team_name_en, expected.team_name_en);
    }
    assert_eq!(
        reopened.root(),
        std::fs::canonicalize(&root).context(IoSnafu)?
    );
    Ok(())
}

#[test]
fn create_never_overwrites_existing_targets_or_neighbor_files() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let roster = Roster::parse(FIXTURE).context(RosterSnafu)?;
    let neighbor = directory.path().join("neighbor.bin");
    std::fs::write(&neighbor, b"keep").context(IoSnafu)?;
    let root = directory.path().join("workspace");
    drop(Workspace::create(&root, &roster).context(WorkspaceSnafu)?);
    let database = root.join("workspace.sqlite3");
    let before = std::fs::read(&database).context(IoSnafu)?;
    assert!(matches!(
        Workspace::create(&root, &roster),
        Err(WorkspaceError::Exists)
    ));
    assert!(matches!(
        Workspace::create(&neighbor, &roster),
        Err(WorkspaceError::Exists)
    ));
    assert_eq!(std::fs::read(database).context(IoSnafu)?, before);
    assert_eq!(std::fs::read(neighbor).context(IoSnafu)?, b"keep");
    Ok(())
}

#[test]
fn invalid_paths_and_file_failures_never_create_a_ready_workspace() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let roster = Roster::parse(FIXTURE).context(RosterSnafu)?;
    assert!(matches!(
        Workspace::create(std::path::Path::new("relative"), &roster),
        Err(WorkspaceError::Path)
    ));
    let escaping = directory.path().join("..").join("not-created");
    assert!(matches!(
        Workspace::create(&escaping, &roster),
        Err(WorkspaceError::Path)
    ));
    let missing_parent = directory.path().join("missing").join("workspace");
    assert!(matches!(
        Workspace::create(&missing_parent, &roster),
        Err(WorkspaceError::Io { .. })
    ));
    assert!(!directory.path().join("missing").exists());
    Ok(())
}

#[test]
fn open_rejects_missing_and_corrupt_databases_without_creating_one() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    assert!(matches!(
        Workspace::open(directory.path()),
        Err(WorkspaceError::Path)
    ));
    let database = directory.path().join("workspace.sqlite3");
    assert!(!database.exists());
    std::fs::write(&database, b"not-a-database").context(IoSnafu)?;
    assert!(matches!(
        Workspace::open(directory.path()),
        Err(WorkspaceError::Database { .. } | WorkspaceError::Connection { .. })
    ));
    assert_eq!(std::fs::read(database).context(IoSnafu)?, b"not-a-database");
    Ok(())
}

#[test]
fn create_and_inspect_work_in_independent_processes() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let source = directory.path().join("roster.json");
    let root = directory.path().join("workspace");
    std::fs::write(&source, FIXTURE).context(IoSnafu)?;
    let executable = env!("CARGO_BIN_EXE_natsume-photo-client");
    let created = Command::new(executable)
        .arg("create-workspace")
        .arg(&root)
        .arg("--roster")
        .arg(&source)
        .output()
        .context(IoSnafu)?;
    assert!(created.status.success());
    std::fs::remove_file(source).context(IoSnafu)?;
    let inspected = Command::new(executable)
        .arg("inspect-workspace")
        .arg(&root)
        .output()
        .context(IoSnafu)?;
    assert!(inspected.status.success());
    assert!(String::from_utf8_lossy(&inspected.stdout).contains("team_count=2"));
    let duplicate = Command::new(executable)
        .arg("create-workspace")
        .arg(&root)
        .arg("--roster")
        .arg(directory.path().join("missing.json"))
        .output()
        .context(IoSnafu)?;
    assert!(!duplicate.status.success());
    let invalid = Command::new(executable)
        .arg("inspect-workspace")
        .arg(directory.path().join("missing"))
        .output()
        .context(IoSnafu)?;
    assert!(!invalid.status.success());
    Ok(())
}
