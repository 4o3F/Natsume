//! Explicit file retirement after confirmed groups drain; never recursively delete directories.
use super::{CaptureState, FinalPhotoError};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{collections::BTreeSet, fs, path::PathBuf};

#[derive(Debug, Snafu)]
pub enum CleanupError {
    #[snafu(display("could not validate current final references before cleanup"))]
    Finals { source: FinalPhotoError },
    #[snafu(display("could not remove a retired workspace entry"))]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("retired workspace entry is no longer at its expected path"))]
    Path { path: PathBuf },
}

/// Cleanup failure does not undo confirmation or prevent the next group.
#[derive(Debug, Default)]
pub struct CleanupReport {
    pub removed_files: usize,
    pub pending_paths: Vec<PathBuf>,
    pub failures: Vec<CleanupError>,
}

#[derive(Default)]
pub(super) struct Cleanup {
    files: BTreeSet<PathBuf>,
    directories: BTreeSet<PathBuf>,
    finals: BTreeSet<PathBuf>,
    pub(super) failures: usize,
}
impl Cleanup {
    pub(super) fn pending(&self) -> usize {
        self.files.len() + self.directories.len() + self.finals.len()
    }
}

impl CaptureState {
    pub(super) fn finish_and_clean(
        &mut self,
        group: crate::capture::GroupId,
        monitor_available: bool,
    ) -> super::Result<CleanupReport> {
        ensure!(monitor_available, super::MonitorUnavailableSnafu);
        self.reconcile();
        self.session
            .finish_group(group)
            .context(super::CaptureSnafu)?;
        self.retire_group_files(group);
        self.confirmed = None;
        self.manual_selection = None;
        Ok(self.clean_retired())
    }
    pub(super) fn retire_group_files(&mut self, group: crate::capture::GroupId) {
        for photo in self
            .ingestion
            .photos()
            .iter()
            .filter(|p| p.group_id == Some(group))
        {
            self.cleanup.files.insert(photo.path.clone());
            if let Some(check) = self.photo_checks.remove(&photo.id)
                && let Ok(candidate) = check.result()
            {
                self.cleanup
                    .files
                    .insert(candidate.image().path().to_owned());
                self.cleanup
                    .files
                    .insert(candidate.thumbnail().path().to_owned());
                if let Some(parent) = candidate.image().path().parent() {
                    self.cleanup.directories.insert(parent.to_owned());
                }
            }
        }
        self.ingestion.retire_group(group);
    }

    pub(super) fn clean_retired(&mut self) -> CleanupReport {
        let mut report = CleanupReport::default();
        // Validation and removal share the final-publication fence with all sessions.
        match self.finals.remove_unused() {
            Ok((removed, failures)) => {
                report.removed_files += removed;
                self.cleanup.finals.clear();
                for (path, source) in failures {
                    self.cleanup.finals.insert(path.clone());
                    report.failures.push(CleanupError::Io { path, source });
                }
            }
            Err(source) => {
                tracing::warn!(
                    "Photo cleanup deferred because current finals could not be validated"
                );
                self.cleanup.failures = 1;
                report.failures.push(CleanupError::Finals { source });
                report
                    .pending_paths
                    .extend(self.cleanup.files.iter().cloned());
                report
                    .pending_paths
                    .extend(self.cleanup.directories.iter().cloned());
                report
                    .pending_paths
                    .extend(self.cleanup.finals.iter().cloned());
                return report;
            }
        }
        let mut live = BTreeSet::new();
        for photo in self.ingestion.photos() {
            live.insert(photo.path.clone());
            if let Some(check) = self.photo_checks.get(&photo.id)
                && let Ok(candidate) = check.result()
            {
                live.insert(candidate.image().path().to_owned());
                live.insert(candidate.thumbnail().path().to_owned());
            }
        }
        self.cleanup.files.retain(|path| {
            if live.contains(path) {
                return true;
            }
            match remove_entry(path, false) {
                Ok(removed) => {
                    report.removed_files += usize::from(removed);
                    false
                }
                Err(error) => {
                    report.failures.push(error);
                    true
                }
            }
        });
        self.cleanup
            .directories
            .retain(|path| match remove_entry(path, true) {
                Ok(_) => false,
                Err(error) => {
                    report.failures.push(error);
                    true
                }
            });
        report
            .pending_paths
            .extend(self.cleanup.files.iter().cloned());
        report
            .pending_paths
            .extend(self.cleanup.directories.iter().cloned());
        report
            .pending_paths
            .extend(self.cleanup.finals.iter().cloned());
        self.cleanup.failures = report.failures.len();
        if !report.failures.is_empty() {
            tracing::warn!(
                failures = report.failures.len(),
                pending = report.pending_paths.len(),
                "Photo cleanup incomplete; confirmed results remain saved"
            );
        }
        report
    }
}

fn remove_entry(path: &PathBuf, directory: bool) -> Result<bool, CleanupError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(CleanupError::Io {
                path: path.clone(),
                source,
            });
        }
    };
    ensure!(
        !metadata.is_symlink()
            && metadata.is_dir() == directory
            && (directory || metadata.is_file())
            && fs::canonicalize(path).context(IoSnafu { path: path.clone() })? == *path,
        PathSnafu { path: path.clone() }
    );
    if directory {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
    .context(IoSnafu { path: path.clone() })?;
    Ok(true)
}
