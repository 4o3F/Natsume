use std::{
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink},
    time::Duration,
};

use diesel::{
    Connection as _, QueryDsl, QueryableByName, RunQueryDsl as _, SqliteConnection,
    connection::SimpleConnection as _,
    sql_query,
    sql_types::{BigInt, Text},
};
use sha2::{Digest as _, Sha256};
use snafu::Snafu;

use super::{Fixture, pdf, registration};
use crate::{
    component::print::{
        PrintComponent, PrintError, PrintState,
        storage::{DocumentStore, StorageError},
    },
    db::{Database, DatabaseConfig, DatabaseError},
    diesel_schema::print_documents,
};

#[derive(Debug, Snafu)]
enum CheckError {
    #[snafu(context(false), display("test timed out"))]
    Elapsed { source: tokio::time::error::Elapsed },
    #[snafu(context(false), display("test filesystem operation failed"))]
    Io { source: std::io::Error },
    #[snafu(context(false), display("test SQL operation failed"))]
    Sql { source: diesel::result::Error },
    #[snafu(context(false), display("test database connection failed"))]
    Connection { source: diesel::ConnectionError },
    #[snafu(context(false), display("test database migration failed"))]
    Database { source: DatabaseError },
    #[snafu(context(false), display("test print operation failed"))]
    Print { source: PrintError },
    #[snafu(context(false), display("test document storage failed"))]
    Storage { source: StorageError },
    #[snafu(context(false), display("test task failed"))]
    Join {
        #[snafu(source(from(tokio::task::JoinError, crate::task::TaskFailure::from)))]
        source: crate::task::TaskFailure,
    },
    #[snafu(display("test filesystem path is not UTF-8"))]
    Path,
    #[snafu(display("document was not persisted before the database write lock"))]
    Timeout,
}

type CheckResult = Result<(), CheckError>;

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

#[tokio::test]
async fn admission_stores_private_immutable_file_and_metadata_only() -> CheckResult {
    let f = Fixture::new().await;
    let id = f.register().await.request_id;
    let bytes = pdf(2);
    let hash = hex::encode(Sha256::digest(&bytes));
    let first = f
        .print
        .upload(f.source.clone(), id.clone(), bytes.clone())
        .await?;
    let path = f.documents_path().join(format!("{hash}.pdf"));
    assert_eq!(fs::read(&path)?, bytes);
    let metadata = fs::metadata(&path)?;
    assert_eq!(metadata.mode() & 0o777, 0o600);
    let replay = f.print.upload(f.source.clone(), id, bytes).await?;
    assert_eq!(replay, first);
    assert_eq!(fs::metadata(&path)?.ino(), metadata.ino());
    let mut connection = SqliteConnection::establish(f.path.to_str().ok_or(CheckError::Path)?)?;
    // Inspect SQLite's physical table independently of the generated Diesel schema.
    assert_eq!(
        sql_query(
            "SELECT count(*) AS n FROM pragma_table_info('print_documents') WHERE name = 'pdf'"
        )
        .get_result::<Count>(&mut connection)?
        .n,
        0
    );
    Ok(())
}

#[tokio::test]
async fn failed_metadata_commit_leaves_reusable_file_without_admitting_request() -> CheckResult {
    let f = Fixture::new().await;
    let id = f.register().await.request_id;
    let bytes = pdf(1);
    let hash = hex::encode(Sha256::digest(&bytes));
    let mut connection = SqliteConnection::establish(f.path.to_str().ok_or(CheckError::Path)?)?;
    connection.batch_execute(
        "CREATE TRIGGER fail_file_admission AFTER INSERT ON print_documents \
         BEGIN SELECT RAISE(ABORT, 'injected'); END;",
    )?;
    assert!(matches!(
        f.print
            .upload(f.source.clone(), id.clone(), bytes.clone())
            .await,
        Err(PrintError::Persistence { .. })
    ));
    let path = f.documents_path().join(format!("{hash}.pdf"));
    assert_eq!(fs::read(&path)?, bytes);
    let inode = fs::metadata(&path)?.ino();
    assert_eq!(
        f.print.get(f.source.clone(), id.clone()).await?.state,
        PrintState::Preparing
    );
    connection.batch_execute("DROP TRIGGER fail_file_admission;")?;
    assert_eq!(
        f.print
            .upload(f.source, id, bytes)
            .await?
            .admission
            .map(|a| a.ready_sequence),
        Some(1)
    );
    assert_eq!(fs::metadata(path)?.ino(), inode);
    Ok(())
}

#[tokio::test]
async fn file_failure_never_commits_ready_metadata() -> CheckResult {
    let f = Fixture::new().await;
    let id = f.register().await.request_id;
    let directory = f.documents_path();
    fs::remove_dir(&directory)?;
    fs::write(&directory, b"not a directory")?;
    assert!(matches!(
        f.print.upload(f.source.clone(), id.clone(), pdf(1)).await,
        Err(PrintError::Storage { .. })
    ));
    assert_eq!(
        f.print.get(f.source, id).await?.state,
        PrintState::Preparing
    );
    Ok(())
}

#[tokio::test]
async fn database_only_restore_fails_until_matching_document_files_are_restored() -> CheckResult {
    let f = Fixture::new().await;
    let id = f.register().await.request_id;
    let bytes = pdf(2);
    let first = f
        .print
        .upload(f.source.clone(), id.clone(), bytes.clone())
        .await?;
    let backup = tempfile::tempdir()?;
    let path = backup.path().join("restored.sqlite3");
    let mut connection = SqliteConnection::establish(f.path.to_str().ok_or(CheckError::Path)?)?;
    // Create a consistent database snapshot after admission, with no concurrent writers.
    sql_query("VACUUM INTO ?")
        .bind::<Text, _>(path.to_str().ok_or(CheckError::Path)?)
        .execute(&mut connection)?;
    let restored = Database::connect_and_migrate(&DatabaseConfig::new(&path, false)).await?;
    let root = backup.path().join("documents");
    assert!(
        PrintComponent::load(restored.clone(), root.clone())
            .await
            .is_err()
    );
    let hash = hex::encode(Sha256::digest(&bytes));
    let stored = fs::read(f.documents_path().join(format!("{hash}.pdf")))?;
    DocumentStore::open(root.clone())?.put(&hash, &stored)?;
    let print = PrintComponent::load(restored, root).await?;
    assert_eq!(print.get(f.source.clone(), id.clone()).await?, first);
    assert_eq!(print.upload(f.source, id, bytes).await?, first);
    Ok(())
}

#[tokio::test]
async fn missing_or_corrupt_referenced_file_fails_startup_and_ready_queries() -> CheckResult {
    let f = Fixture::new().await;
    let input = registration();
    f.print.register(f.source.clone(), input.clone()).await?;
    let bytes = pdf(1);
    let hash = hex::encode(Sha256::digest(&bytes));
    f.print
        .upload(f.source.clone(), input.request_id.clone(), bytes.clone())
        .await?;
    let root = f.documents_path();
    let path = root.join(format!("{hash}.pdf"));
    fs::remove_file(&path)?;
    assert!(
        PrintComponent::load(f.database.clone(), root.clone())
            .await
            .is_err()
    );
    assert!(matches!(
        f.print
            .get(f.source.clone(), input.request_id.clone())
            .await,
        Err(PrintError::Storage { .. })
    ));
    assert!(matches!(
        f.print.register(f.source.clone(), input).await,
        Err(PrintError::Storage { .. })
    ));
    f.print.documents.put(&hash, &bytes)?;
    fs::write(&path, b"corrupted")?;
    assert!(
        PrintComponent::load(f.database.clone(), root)
            .await
            .is_err()
    );
    assert!(f.print.documents.put(&hash, &bytes).is_err());
    assert_eq!(fs::read(path)?, b"corrupted");
    Ok(())
}

#[tokio::test]
async fn durable_file_write_does_not_hold_or_require_sqlite_write_lock() -> CheckResult {
    let f = Fixture::new().await;
    let id = f.register().await.request_id;
    let bytes = pdf(1);
    let path = f
        .documents_path()
        .join(format!("{}.pdf", hex::encode(Sha256::digest(&bytes))));
    let mut connection = SqliteConnection::establish(f.path.to_str().ok_or(CheckError::Path)?)?;
    connection.batch_execute("BEGIN IMMEDIATE;")?;
    let print = PrintComponent::for_test(f.database.clone(), f.documents_path())?;
    let source = f.source.clone();
    let task = tokio::spawn(async move { print.upload(source, id, bytes).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(CheckError::from)?;
    assert!(!task.is_finished());
    assert_eq!(
        print_documents::table
            .count()
            .get_result::<i64>(&mut connection)?,
        0
    );
    connection.batch_execute("COMMIT;")?;
    assert_eq!(task.await??.state, PrintState::Ready);
    Ok(())
}

#[test]
fn document_store_rejects_symlinks_and_unsafe_directories() -> CheckResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("documents");
    let store = DocumentStore::open(root.clone())?;
    let bytes = pdf(1);
    let hash = hex::encode(Sha256::digest(&bytes));
    let outside = directory.path().join("outside.pdf");
    fs::write(&outside, &bytes)?;
    symlink(&outside, root.join(format!("{hash}.pdf")))?;
    assert!(store.put(&hash, &bytes).is_err());
    assert_eq!(fs::read(outside)?, bytes);
    assert!(store.put("../outside", b"data").is_err());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777))?;
    assert!(DocumentStore::open(root).is_err());
    Ok(())
}
