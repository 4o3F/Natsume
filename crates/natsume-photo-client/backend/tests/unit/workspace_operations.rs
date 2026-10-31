use crate::{
    roster::Roster,
    workspace::{Error as WorkspaceError, Workspace},
};
use snafu::{ResultExt as _, Snafu, ensure};

const FIXTURE: &[u8] = include_bytes!("../fixtures/roster.json");

#[derive(Debug, Snafu)]
enum Error {
    Io {
        source: std::io::Error,
    },
    Roster {
        source: crate::roster::Error,
    },
    Workspace {
        source: WorkspaceError,
    },
    Persistence {
        source: crate::workspace::PersistenceError,
    },
    Channel {
        source: std::sync::mpsc::RecvTimeoutError,
    },
    FinalPhoto {
        source: crate::workspace::FinalPhotoError,
    },
    Thread,
}
type Result<T = ()> = std::result::Result<T, Error>;

#[test]
fn final_cleanup_waits_for_the_shared_publication_fence() -> Result {
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let roster = Roster::parse(FIXTURE).context(RosterSnafu)?;
    let owner = Workspace::create_with_logos(
        directory.path().join("workspace"),
        roster.teams().to_vec(),
        directory.path(),
        Vec::new(),
    )
    .context(WorkspaceSnafu)?;
    let store = owner.final_store();
    let publication = owner
        .database
        .lock_final_operations()
        .context(PersistenceSnafu)?;
    let (entered, ready) = mpsc::channel();
    let (sent, result) = mpsc::channel();
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            let _ = entered.send(());
            let _ = sent.send(store.remove_unused());
        });
        ready
            .recv_timeout(Duration::from_secs(5))
            .context(ChannelSnafu)?;
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Timeout)
        ));
        drop(publication);
        let (removed, failures) = result
            .recv_timeout(Duration::from_secs(5))
            .context(ChannelSnafu)?
            .context(FinalPhotoSnafu)?;
        assert_eq!(removed, 0);
        assert!(failures.is_empty());
        ensure!(worker.join().is_ok(), ThreadSnafu);
        Ok(())
    })?;
    Ok(())
}

#[tokio::test]
async fn only_one_writable_owner_is_allowed_but_readonly_diagnosis_can_open() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let roster = Roster::parse(FIXTURE).context(RosterSnafu)?;
    let root = directory.path().join("workspace");
    let owner = crate::workspace::fixture::create(&root, &roster)
        .await
        .context(WorkspaceSnafu)?;
    assert!(matches!(
        Workspace::open(&root).await,
        Err(WorkspaceError::Storage {
            source: crate::workspace::StorageError::Busy
        })
    ));
    drop(
        crate::workspace::WorkspaceReader::inspect(&root)
            .await
            .context(WorkspaceSnafu)?,
    );
    drop(owner);
    drop(Workspace::open(&root).await.context(WorkspaceSnafu)?);
    Ok(())
}

#[tokio::test]
async fn roster_survives_close_reopen_and_source_removal() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let source = directory.path().join("roster.json");
    std::fs::write(&source, FIXTURE).context(IoSnafu)?;
    let roster = Roster::load(&source).context(RosterSnafu)?;
    let root = directory.path().join("工作区 #1% 比赛");
    drop(
        crate::workspace::fixture::create(&root, &roster)
            .await
            .context(WorkspaceSnafu)?,
    );
    std::fs::remove_file(&source).context(IoSnafu)?;
    let reopened = Workspace::open(&root).await.context(WorkspaceSnafu)?;
    let persisted = reopened.roster().await.context(WorkspaceSnafu)?;
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

#[tokio::test]
async fn create_never_overwrites_existing_targets_or_neighbor_files() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let roster = Roster::parse(FIXTURE).context(RosterSnafu)?;
    let neighbor = directory.path().join("neighbor.bin");
    std::fs::write(&neighbor, b"keep").context(IoSnafu)?;
    let root = directory.path().join("workspace");
    drop(
        crate::workspace::fixture::create(&root, &roster)
            .await
            .context(WorkspaceSnafu)?,
    );
    let database = root.join("workspace.sqlite3");
    let before = std::fs::read(&database).context(IoSnafu)?;
    assert!(matches!(
        crate::workspace::fixture::create(&root, &roster).await,
        Err(WorkspaceError::Storage {
            source: crate::workspace::StorageError::Exists
        })
    ));
    assert!(matches!(
        crate::workspace::fixture::create(&neighbor, &roster).await,
        Err(WorkspaceError::Storage {
            source: crate::workspace::StorageError::Exists
        })
    ));
    assert_eq!(std::fs::read(database).context(IoSnafu)?, before);
    assert_eq!(std::fs::read(neighbor).context(IoSnafu)?, b"keep");
    Ok(())
}

#[tokio::test]
async fn invalid_paths_and_file_failures_never_create_a_ready_workspace() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let roster = Roster::parse(FIXTURE).context(RosterSnafu)?;
    assert!(matches!(
        crate::workspace::fixture::create(std::path::Path::new("relative"), &roster).await,
        Err(WorkspaceError::Storage {
            source: crate::workspace::StorageError::Path
        })
    ));
    let escaping = directory.path().join("..").join("not-created");
    assert!(matches!(
        crate::workspace::fixture::create(&escaping, &roster).await,
        Err(WorkspaceError::Storage {
            source: crate::workspace::StorageError::Path
        })
    ));
    let missing_parent = directory.path().join("missing").join("workspace");
    assert!(matches!(
        crate::workspace::fixture::create(&missing_parent, &roster).await,
        Err(WorkspaceError::Storage {
            source: crate::workspace::StorageError::Io { .. }
        })
    ));
    assert!(!directory.path().join("missing").exists());
    Ok(())
}

#[tokio::test]
async fn open_rejects_missing_and_corrupt_databases_without_creating_one() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    assert!(matches!(
        Workspace::open(directory.path()).await,
        Err(WorkspaceError::Storage {
            source: crate::workspace::StorageError::Io { .. }
        })
    ));
    let database = directory.path().join("workspace.sqlite3");
    assert!(!database.exists());
    std::fs::write(&database, b"not-a-database").context(IoSnafu)?;
    assert!(matches!(
        Workspace::open(directory.path()).await,
        Err(WorkspaceError::Persistence { .. } | WorkspaceError::Storage { .. })
    ));
    assert_eq!(std::fs::read(database).context(IoSnafu)?, b"not-a-database");
    Ok(())
}
