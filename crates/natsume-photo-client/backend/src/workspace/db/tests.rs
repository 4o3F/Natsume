use super::*;
use crate::{
    roster::Roster,
    workspace::{Error, Workspace, storage},
};
use diesel::connection::SimpleConnection as _;

#[derive(Debug, Snafu)]
enum TestError {
    Io { source: std::io::Error },
    Storage { source: storage::StorageError },
    Persistence { source: PersistenceError },
    Roster { source: crate::roster::Error },
}
type TestResult<T = ()> = std::result::Result<T, TestError>;

fn roster() -> TestResult<Roster> {
    Roster::parse(include_bytes!("../../../tests/fixtures/roster.json")).context(RosterSnafu)
}
async fn unready() -> TestResult<(tempfile::TempDir, std::path::PathBuf, Database)> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let (root, database) = storage::create(directory.path().join("workspace"))
        .await
        .context(StorageSnafu)?;
    Ok((directory, root, database))
}

#[tokio::test]
async fn incomplete_workspace_is_rejected() -> TestResult {
    let (_directory, root, _database) = unready().await?;
    assert!(matches!(
        Workspace::open(&root).await,
        Err(Error::Incomplete)
    ));
    Ok(())
}

#[tokio::test]
async fn failed_roster_write_rolls_back_all_rows_and_ready_flag() -> TestResult {
    let (_directory, root, database) = unready().await?;
    // Fault injection: fail after the first row to verify the transaction boundary.
    database
        .write(|tx| {
            tx.connection().batch_execute(
                "CREATE TRIGGER fail_second_insert BEFORE INSERT ON photo_roster
            WHEN NEW.account = 'T-002' BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
            )?;
            Ok(())
        })
        .await
        .context(PersistenceSnafu)?;
    assert!(matches!(
        database
            .save_roster(roster()?.teams().to_vec(), Cache::default())
            .await,
        Err(PersistenceError::Query { .. })
    ));
    let count: i64 = database
        .read(|tx| Ok(photo_roster::table.count().get_result(tx.connection())?))
        .await
        .context(PersistenceSnafu)?;
    assert_eq!(count, 0);
    assert_eq!(
        database.metadata().await.context(PersistenceSnafu)?.ready,
        0
    );
    assert!(matches!(
        Workspace::open(&root).await,
        Err(Error::Incomplete)
    ));
    Ok(())
}

#[tokio::test]
async fn readiness_and_persisted_roster_are_validated_in_rust() -> TestResult {
    let (_directory, root, database) = unready().await?;
    database
        .save_roster(roster()?.teams().to_vec(), Cache::default())
        .await
        .context(PersistenceSnafu)?;
    database
        .write(|tx| {
            diesel::update(photo_workspace::table.find(1))
                .set(photo_workspace::ready.eq(2))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .context(PersistenceSnafu)?;
    assert!(matches!(
        Workspace::open(&root).await,
        Err(Error::InvalidReadiness)
    ));
    database
        .write(|tx| {
            diesel::update(photo_workspace::table.find(1))
                .set(photo_workspace::ready.eq(1))
                .execute(tx.connection())?;
            diesel::update(photo_roster::table.filter(photo_roster::account.eq("T-001")))
                .set(photo_roster::account_id.eq(" "))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .context(PersistenceSnafu)?;
    assert!(matches!(
        Workspace::open(&root).await,
        Err(Error::Roster {
            source: crate::roster::Error::MissingIdentity { .. }
        })
    ));
    database
        .write(|tx| {
            diesel::delete(photo_roster::table).execute(tx.connection())?;
            Ok(())
        })
        .await
        .context(PersistenceSnafu)?;
    assert!(matches!(
        Workspace::open(&root).await,
        Err(Error::Roster {
            source: crate::roster::Error::Empty
        })
    ));
    Ok(())
}

#[test]
fn connection_never_creates_missing_database() -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let path = directory.path().join("missing.sqlite3");
    assert!(matches!(
        Database::connect(&path),
        Err(PersistenceError::Connection { .. })
    ));
    assert!(!path.exists());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn database_work_does_not_block_the_async_runtime() -> TestResult {
    let (_directory, _root, database) = unready().await?;
    let (release, waiting) = std::sync::mpsc::channel();
    let operation = database.read(move |_| {
        // A bounded wait prevents a broken implementation from hanging the suite.
        Ok(waiting
            .recv_timeout(std::time::Duration::from_secs(2))
            .is_ok())
    });
    let other_task = async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let _ = release.send(());
    };
    let (released_by_async_task, ()) = tokio::join!(operation, other_task);
    assert!(released_by_async_task.context(PersistenceSnafu)?);
    Ok(())
}
