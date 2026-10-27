use std::{sync::mpsc, time::Duration};

use super::*;

type TestResult = Result<(), crate::test_error::TestError>;

#[tokio::test]
async fn import_capacity_is_shared_and_lightweight_operations_remain_available() {
    let fixture = Fixture::new().await;
    let operation = checked(fixture.import.try_begin());
    assert!(matches!(fixture.import.try_begin(), Err(ImportError::Busy)));
    assert!(checked(fixture.import.read_pending().await).is_none());
    drop(operation);
    assert!(fixture.import.try_begin().is_ok());
    assert!(
        checked(fixture.import.try_begin())
            .create_candidate(b"invalid")
            .await
            .is_err()
    );
    assert!(fixture.import.try_begin().is_ok());
}

#[test]
fn cancelled_queued_parse_keeps_capacity_until_the_blocking_task_exits() -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()?;
    runtime.block_on(async {
        let fixture = Fixture::new().await;
        let (release, waiting) = mpsc::channel::<()>();
        let (entered, started) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = entered.send(());
            let _ = waiting.recv_timeout(Duration::from_secs(10));
        });
        tokio::time::timeout(Duration::from_secs(5), started).await??;
        let bytes = workbook(&[row("School", "team-1", "A-1", "synthetic-password")]);
        let operation = fixture.import.try_begin()?;
        let mut parsing = Box::pin(operation.create_candidate(&bytes));
        assert!(futures_util::poll!(parsing.as_mut()).is_pending());
        drop(parsing);
        assert!(matches!(fixture.import.try_begin(), Err(ImportError::Busy)));
        drop(release);
        blocker.await?;
        let restored =
            tokio::time::timeout(Duration::from_secs(5), fixture.import.work.acquire()).await??;
        drop(restored);
        assert!(fixture.import.read_pending().await?.is_none());
        Ok(())
    })
}

#[tokio::test]
async fn cancelled_write_keeps_capacity_and_does_not_claim_to_undo_a_transaction() -> TestResult {
    for fail in [false, true] {
        let fixture = Fixture::new().await;
        fixture
            .execute("CREATE TABLE import_capacity_evidence (value INTEGER NOT NULL);")
            .await;
        let operation = fixture.import.try_begin()?;
        let (release, waiting) = mpsc::channel::<()>();
        let (entered, started) = tokio::sync::oneshot::channel();
        let mut transaction_work = Box::pin(super::super::write_import(
            &fixture.database,
            operation.permit,
            move |transaction| {
                transaction
                    .connection()
                    .batch_execute("INSERT INTO import_capacity_evidence VALUES (1);")
                    .map_err(|source| ImportError::PersistenceFailure {
                        source: source.into(),
                    })?;
                let _ = entered.send(());
                let _ = waiting.recv_timeout(Duration::from_secs(10));
                if fail {
                    Err(ImportError::PersistenceFailure {
                        source: crate::db::PersistenceError::invalid("persisted facts"),
                    })
                } else {
                    Ok(())
                }
            },
        ));
        assert!(futures_util::poll!(transaction_work.as_mut()).is_pending());
        tokio::time::timeout(Duration::from_secs(5), started).await??;
        drop(transaction_work);
        assert!(matches!(fixture.import.try_begin(), Err(ImportError::Busy)));
        drop(release);
        let restored =
            tokio::time::timeout(Duration::from_secs(5), fixture.import.work.acquire()).await??;
        drop(restored);
        let rows = fixture
            .database
            .read(|transaction| {
                diesel::sql_query("SELECT COUNT(*) AS value FROM import_capacity_evidence")
                    .get_result::<IntegerValue>(transaction.connection())
                    .map(|row| row.value)
                    .map_err(|source| ImportError::PersistenceFailure {
                        source: source.into(),
                    })
            })
            .await
            .map_err(crate::db::TransactionError::into_error)?;
        assert_eq!(rows, i64::from(!fail));
    }
    Ok(())
}
