use super::*;
use crate::test_error::TestError;

#[tokio::test]
async fn budget_samples_are_cached_until_the_interval_expires() -> Result<(), TestError> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("journal.db");
    std::fs::write(&path, [0; 80])?;
    let mut writer = Writer {
        path: path.clone(),
        config: JournalConfig {
            max_bytes: 100,
            min_free_bytes: 1,
            ..JournalConfig::default()
        },
        stats: Arc::new(JournalStats::default()),
        boot_id: Uuid::now_v7(),
        database: None,
        pause_marker: None,
        reported_drops: PerCategory::default(),
        budget_sample: None,
        row_estimate: None,
    };
    assert!(writer.check_budget().await);
    tokio::time::pause();
    std::fs::write(&path, [0; 100])?;
    assert!(writer.check_budget().await);
    tokio::time::advance(BUDGET_CHECK_INTERVAL).await;
    assert!(!writer.check_budget().await);
    assert_eq!(
        writer.stats.pause_reason.load(),
        Some(PauseReason::DiskBudget)
    );
    std::fs::write(&path, [0; 10])?;
    assert!(!writer.check_budget().await);
    tokio::time::advance(PAUSE_RECOVERY_INTERVAL).await;
    assert!(writer.refresh_budget().await);
    assert_eq!(writer.stats.pause_reason.load(), None);
    Ok(())
}

#[cfg(unix)]
#[test]
fn disk_usage_preserves_non_utf8_sidecar_paths() -> Result<(), TestError> {
    use std::os::unix::ffi::OsStringExt as _;
    let root = tempfile::tempdir()?;
    let path = root
        .path()
        .join(std::ffi::OsString::from_vec(b"journal-\xff.db".to_vec()));
    std::fs::write(&path, [0; 5])?;
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    std::fs::write(&wal, [0; 7])?;
    let mut shm = path.as_os_str().to_owned();
    shm.push("-shm");
    std::fs::write(&shm, [0; 9])?;
    let usage = disk_sizes(&path).unwrap_or_else(|error| panic!("disk usage: {error}"));
    assert_eq!(usage.db_bytes, 5);
    assert_eq!(usage.wal_bytes, 7);
    assert_eq!(usage.shm_bytes, 9);
    assert!(usage.free_bytes > 0);
    Ok(())
}

fn test_writer(path: PathBuf, database: JournalDatabase) -> Writer {
    Writer {
        path,
        config: JournalConfig {
            min_free_bytes: 1,
            ..JournalConfig::default()
        },
        stats: Arc::new(JournalStats::default()),
        boot_id: Uuid::now_v7(),
        database: Some(database),
        pause_marker: None,
        reported_drops: PerCategory::default(),
        budget_sample: None,
        row_estimate: None,
    }
}

#[test]
fn loss_bound_tracks_the_minimum_across_out_of_order_losses() {
    let stats = JournalStats::default();
    stats.drop_events(&Category::Device, 1, 20);
    stats.drop_events(&Category::Device, 1, 10);
    stats.drop_events(&Category::Device, 1, 30);
    assert_eq!(
        stats.categories.device.first_unreported_drop_ms.load(),
        Some(10)
    );
}

#[tokio::test]
async fn loss_bound_of_a_failed_batch_is_its_oldest_event_occurrence() -> Result<(), TestError> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("journal.db");
    let database = JournalDatabase::open(&path).await?;
    super::super::db::tests::fast_busy_timeout(&database)?;
    let mut writer = test_writer(path.clone(), database.clone());
    let lock = crate::db::tests::test_lock_database(&path)?;
    writer
        .stats
        .categories
        .device
        .queued
        .store(2, Ordering::Relaxed);
    writer
        .batch(
            [10, 15]
                .into_iter()
                .map(|at| Envelope {
                    event: Event::LeaseAttached(crate::journal::event::payload::LeaseAttached {
                        session_id: crate::journal::event::types::SessionId::from_lease(
                            Uuid::now_v7(),
                        ),
                    }),
                    context: JournalContext::default(),
                    occurred_at_ms: at,
                })
                .collect(),
        )
        .await;
    drop(lock);
    writer.report_drops().await;
    let rows = database.read_cursor(None, None, 100).await?;
    let row = rows
        .iter()
        .find(|row| row.1.kind == "journal_dropped")
        .ok_or(TestError::missing("drop marker"))?;
    let crate::journal::event::ReadEvent::Known(Event::JournalDropped(marker)) =
        crate::journal::event::ReadEvent::from_persisted(row.0, &row.1.kind, &row.1.details)
    else {
        panic!("typed drop marker");
    };
    assert_eq!(marker.since, 10);
    assert_eq!(marker.count, 2);
    Ok(())
}

#[tokio::test]
async fn row_estimate_avoids_count_per_batch_while_enforcing_the_row_limit() -> Result<(), TestError>
{
    let root = tempfile::tempdir()?;
    let path = root.path().join("journal.db");
    let database = JournalDatabase::open(&path).await?;
    let mut writer = test_writer(path, database.clone());
    writer.config.max_rows = 2;
    writer.retention_cycle().await;
    let queries = super::super::db::tests::capture_queries(&database);
    writer.retain().await;
    for _ in 0..3 {
        writer
            .stats
            .categories
            .device
            .queued
            .fetch_add(1, Ordering::Relaxed);
        writer
            .batch(vec![Envelope {
                event: Event::LeaseAttached(crate::journal::event::payload::LeaseAttached {
                    session_id: crate::journal::event::types::SessionId::from_lease(Uuid::now_v7()),
                }),
                context: JournalContext::default(),
                occurred_at_ms: now_ms(),
            }])
            .await;
    }
    assert_eq!(database.read_cursor(None, None, 100).await?.len(), 2);
    {
        let queries = queries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            !queries
                .iter()
                .any(|query| query.to_ascii_lowercase().contains("count(")),
            "normal batches must not count: {queries:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn row_estimate_is_recalibrated_and_tracks_size_pruning_and_direct_markers()
-> Result<(), TestError> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("journal.db");
    let database = JournalDatabase::open(&path).await?;
    let mut writer = test_writer(path, database.clone());
    writer.retention_cycle().await;
    assert_eq!(writer.row_estimate, Some(0));
    assert!(
        writer
            .direct(Event::JournalResumed(JournalResumed {}), now_ms())
            .await
    );
    assert_eq!(writer.row_estimate, Some(1));
    assert_eq!(writer.prune(Prune::OldestBatch).await?, 1);
    assert_eq!(writer.row_estimate, Some(0));
    writer.row_estimate = Some(100);
    writer.retention_cycle().await;
    assert_eq!(writer.row_estimate, Some(0));
    Ok(())
}
