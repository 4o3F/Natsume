use std::path::Path;

use snafu::{ResultExt, Snafu};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BudgetDecision {
    Normal,
    PruneSoon,
    Paused,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::struct_field_names,
    reason = "explicit byte units distinguish disk measurements"
)]
pub(super) struct DiskUsage {
    /// Current main database length in bytes, sampled by the writer.
    pub(super) db_bytes: u64,
    /// Current WAL length in bytes, sampled by the writer; zero if absent.
    pub(super) wal_bytes: u64,
    /// Current shared-memory length in bytes, sampled by the writer; zero if absent.
    pub(super) shm_bytes: u64,
    /// Available filesystem space in bytes, sampled by statvfs.
    pub(super) free_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct BudgetLimits {
    /// Maximum combined database/WAL/shared-memory bytes, fixed by configuration.
    pub(super) max_bytes: u64,
    /// Minimum filesystem space in bytes, fixed by configuration.
    pub(super) min_free_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PauseState {
    Running,
    Paused,
}

/// Percentage denominator used for exact integer budget comparisons.
const PERCENT: u128 = 100;
/// Above this percentage, proactively prune the journal.
const PRUNE_PERCENT: u128 = 80;
/// Below this percentage, a paused journal may resume writing.
const RESUME_PERCENT: u128 = 60;

pub(super) fn decide(usage: DiskUsage, limits: BudgetLimits, state: PauseState) -> BudgetDecision {
    let used =
        u128::from(usage.db_bytes) + u128::from(usage.wal_bytes) + u128::from(usage.shm_bytes);
    let maximum = u128::from(limits.max_bytes);
    if used >= maximum || usage.free_bytes < limits.min_free_bytes {
        BudgetDecision::Paused
    } else if state == PauseState::Paused {
        if used * PERCENT < maximum * RESUME_PERCENT {
            BudgetDecision::Resume
        } else {
            BudgetDecision::Paused
        }
    } else if used * PERCENT > maximum * PRUNE_PERCENT {
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

pub(super) fn free_bytes(path: &Path) -> Result<u64, BudgetError> {
    let stats = rustix::fs::statvfs(path).context(FilesystemSnafu)?;
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_include_wal_shm_free_space_and_hysteresis() {
        let limits = BudgetLimits {
            max_bytes: 100,
            min_free_bytes: 10,
        };
        let usage = DiskUsage {
            db_bytes: 80,
            wal_bytes: 0,
            shm_bytes: 0,
            free_bytes: 10,
        };
        assert_eq!(
            decide(usage, limits, PauseState::Running),
            BudgetDecision::Normal
        );
        assert_eq!(
            decide(
                DiskUsage {
                    db_bytes: 79,
                    wal_bytes: 1,
                    shm_bytes: 1,
                    ..usage
                },
                limits,
                PauseState::Running
            ),
            BudgetDecision::PruneSoon
        );
        assert_eq!(
            decide(
                DiskUsage {
                    db_bytes: 98,
                    wal_bytes: 1,
                    shm_bytes: 1,
                    ..usage
                },
                limits,
                PauseState::Running
            ),
            BudgetDecision::Paused
        );
        assert_eq!(
            decide(
                DiskUsage {
                    db_bytes: 1,
                    free_bytes: 9,
                    ..usage
                },
                limits,
                PauseState::Running
            ),
            BudgetDecision::Paused
        );
        assert_eq!(
            decide(
                DiskUsage {
                    db_bytes: 60,
                    ..usage
                },
                limits,
                PauseState::Paused
            ),
            BudgetDecision::Paused
        );
        assert_eq!(
            decide(
                DiskUsage {
                    db_bytes: 59,
                    ..usage
                },
                limits,
                PauseState::Paused
            ),
            BudgetDecision::Resume
        );
        assert_eq!(
            decide(
                DiskUsage {
                    db_bytes: 59,
                    free_bytes: 9,
                    ..usage
                },
                limits,
                PauseState::Paused
            ),
            BudgetDecision::Paused
        );
    }

    #[test]
    fn combined_file_sizes_do_not_overflow() {
        assert_eq!(
            decide(
                DiskUsage {
                    db_bytes: u64::MAX,
                    wal_bytes: u64::MAX,
                    shm_bytes: u64::MAX,
                    free_bytes: u64::MAX
                },
                BudgetLimits {
                    max_bytes: u64::MAX,
                    min_free_bytes: 1
                },
                PauseState::Running
            ),
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
