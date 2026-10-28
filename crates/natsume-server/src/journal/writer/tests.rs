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
