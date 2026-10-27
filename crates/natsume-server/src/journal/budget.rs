use std::path::Path;

use snafu::{ResultExt, Snafu};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BudgetDecision {
    Normal,
    PruneSoon,
    Paused,
    Resume,
}

#[cfg_attr(not(test), expect(dead_code, reason = "wired in S3"))]
pub(super) fn decide(
    db_bytes: u64,
    wal_bytes: u64,
    shm_bytes: u64,
    free_bytes: u64,
    max_bytes: u64,
    min_free_bytes: u64,
    paused: bool,
) -> BudgetDecision {
    let used = u128::from(db_bytes) + u128::from(wal_bytes) + u128::from(shm_bytes);
    let maximum = u128::from(max_bytes);
    if used >= maximum || free_bytes < min_free_bytes {
        BudgetDecision::Paused
    } else if paused {
        if used * 100 < maximum * 60 {
            BudgetDecision::Resume
        } else {
            BudgetDecision::Paused
        }
    } else if used * 100 > maximum * 80 {
        BudgetDecision::PruneSoon
    } else {
        BudgetDecision::Normal
    }
}

#[derive(Debug, Snafu)]
pub(super) enum BudgetError {
    #[snafu(display("cannot read journal filesystem space"))]
    Filesystem { source: rustix::io::Errno },
}

#[cfg_attr(not(test), expect(dead_code, reason = "wired in S3"))]
pub(super) fn free_bytes(path: &Path) -> Result<u64, BudgetError> {
    let stats = rustix::fs::statvfs(path).context(FilesystemSnafu)?;
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_include_wal_shm_free_space_and_hysteresis() {
        for (db, wal, shm, free, paused, expected) in [
            (80, 0, 0, 10, false, BudgetDecision::Normal),
            (79, 1, 1, 10, false, BudgetDecision::PruneSoon),
            (98, 1, 1, 10, false, BudgetDecision::Paused),
            (1, 0, 0, 9, false, BudgetDecision::Paused),
            (60, 0, 0, 10, true, BudgetDecision::Paused),
            (59, 0, 0, 10, true, BudgetDecision::Resume),
            (59, 0, 0, 9, true, BudgetDecision::Paused),
        ] {
            assert_eq!(decide(db, wal, shm, free, 100, 10, paused), expected);
        }
        assert_eq!(
            decide(u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX, 1, false),
            BudgetDecision::Paused
        );
    }

    #[test]
    fn filesystem_wrapper_reads_available_space_and_preserves_errors() -> Result<(), BudgetError> {
        assert!(free_bytes(Path::new("."))? > 0);
        assert!(matches!(
            free_bytes(Path::new("/nonexistent-natsume-journal")),
            Err(BudgetError::Filesystem { .. })
        ));
        Ok(())
    }
}
