//! File observations and archival, without capture-session mutation authority.
use super::{
    ArchiveError, ArchiveSnafu, ArchivedPhoto, Error, INTERVAL, IngestionState, InvariantSnafu,
    IoSnafu, MissingSource, NotReviewableSnafu, Result, UnsafePathSnafu, archive,
};
use crate::capture::GroupId;
use snafu::{IntoError as _, OptionExt as _, ResultExt as _, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use uuid::Uuid;
struct Pending {
    owner: Option<GroupId>,
    version: Option<archive::SourceFileVersion>,
    stable_since: Instant,
}
pub(super) struct Ingestion {
    hot_folder: PathBuf,
    archive: PathBuf,
    pending: BTreeMap<PathBuf, Pending>,
    seen: BTreeMap<PathBuf, ObservedSource>,
    /// Last content verification for each archived source, used for audit ordering.
    checked: BTreeMap<PathBuf, Instant>,
    /// Global low-priority audit budget, independent of folder size.
    last_audit: Instant,
    /// Session-long tombstones: acknowledging a warning never restores ownership.
    lost: BTreeSet<PathBuf>,
    photos: Vec<ArchivedPhoto>,
    missing_sources: Vec<MissingSource>,
    state: IngestionState,
    uncertain: bool,
}

struct ObservedSource {
    version: archive::SourceFileVersion,
    owner: Option<GroupId>,
}
impl Ingestion {
    pub(super) fn new(hot_folder: PathBuf, archive: PathBuf) -> Self {
        Self {
            hot_folder,
            archive,
            pending: BTreeMap::new(),
            seen: BTreeMap::new(),
            checked: BTreeMap::new(),
            last_audit: Instant::now(),
            lost: BTreeSet::new(),
            photos: Vec::new(),
            missing_sources: Vec::new(),
            state: IngestionState::Running,
            uncertain: false,
        }
    }
    pub(super) fn hot_folder(&self) -> &Path {
        &self.hot_folder
    }
    pub(super) fn archive(&self) -> &Path {
        &self.archive
    }
    pub(super) fn state(&self) -> IngestionState {
        self.state
    }
    pub(super) fn photos(&self) -> &[ArchivedPhoto] {
        &self.photos
    }
    pub(super) fn retire_group(&mut self, group: GroupId) {
        // Keep source observations/tombstones: unchanged A originals must not reappear for B.
        self.photos.retain(|photo| photo.group_id != Some(group));
    }
    pub(super) fn missing_sources(&self) -> &[MissingSource] {
        &self.missing_sources
    }
    pub(super) fn pending_owners(&self) -> impl Iterator<Item = (&PathBuf, GroupId)> {
        self.pending
            .iter()
            .filter_map(|(path, pending)| pending.owner.map(|group| (path, group)))
    }
    pub(super) fn monitor_lost(&mut self) {
        self.state = IngestionState::RecoveringMonitor;
        self.uncertain = true;
    }
    pub(super) fn stop(&mut self) {
        self.state = IngestionState::Stopped;
    }
    pub(super) fn acknowledge_missing(&mut self, path: &Path) -> Result<()> {
        let before = self.missing_sources.len();
        self.missing_sources
            .retain(|missing| missing.source != path);
        ensure!(before != self.missing_sources.len(), NotReviewableSnafu);
        Ok(())
    }
    pub(super) fn assign_reviewed(&mut self, photo: Uuid, group: GroupId) -> Result<()> {
        self.photos
            .iter_mut()
            .find(|p| p.id == photo && p.group_id.is_none())
            .context(NotReviewableSnafu)?
            .group_id = Some(group);
        Ok(())
    }
    pub(super) fn reconcile(&mut self, owner: Option<GroupId>) -> Result<()> {
        match self.scan(owner) {
            Ok(()) => {
                self.state = IngestionState::Running;
                self.uncertain = false;
                Ok(())
            }
            Err(error) => {
                self.state = if matches!(
                    error,
                    Error::UnsafePath
                        | Error::Archive {
                            source: ArchiveError::UnsafePath
                        }
                ) {
                    IngestionState::PausedDirectory
                } else {
                    IngestionState::PausedArchive
                };
                self.uncertain = true;
                Err(error)
            }
        }
    }
    pub(super) fn scan(&mut self, owner: Option<GroupId>) -> Result<()> {
        self.scan_with_probe(owner, archive::probe)
    }
    pub(super) fn scan_with_probe(
        &mut self,
        current: Option<GroupId>,
        mut probe: impl FnMut(&Path, &Path) -> archive::Result<archive::SourceFileVersion>,
    ) -> Result<()> {
        ensure!(
            fs::canonicalize(&self.hot_folder).context(IoSnafu)? == self.hot_folder,
            UnsafePathSnafu
        );
        let mut present = BTreeSet::new();
        for entry in fs::read_dir(&self.hot_folder).context(IoSnafu)? {
            let entry = entry.context(IoSnafu)?;
            let path = entry.path();
            let kind = entry.file_type().context(IoSnafu)?;
            ensure!(kind.is_file() && !kind.is_symlink(), UnsafePathSnafu);
            let supported = path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    ["jpg", "jpeg", "png"]
                        .iter()
                        .any(|ext| value.eq_ignore_ascii_case(ext))
                });
            if !supported {
                continue;
            }
            present.insert(path);
        }
        // Only a complete, valid directory listing can establish source loss.
        // Retire losses before observing new names, so renamed files need review.
        let missing: Vec<_> = self
            .pending
            .keys()
            .filter(|path| !present.contains(*path))
            .cloned()
            .collect();
        for path in missing {
            let pending = self.pending.remove(&path).context(InvariantSnafu)?;
            tracing::warn!("Photo source vanished before archival; photographer review required");
            self.lost.insert(path.clone());
            self.missing_sources
                .retain(|missing| missing.source != path);
            self.missing_sources.push(MissingSource {
                source: path,
                group_id: pending.owner,
            });
            self.uncertain = true;
        }
        // One full-file audit per 30 seconds globally, even with many originals.
        // Normal metadata changes bypass this low-priority content audit budget.
        let audit = (self.last_audit.elapsed() >= Duration::from_secs(30))
            .then(|| {
                self.checked
                    .iter()
                    .filter(|(path, _)| {
                        present.contains(*path) && !self.pending.contains_key(*path)
                    })
                    .min_by_key(|(_, at)| **at)
                    .filter(|(_, at)| at.elapsed() >= Duration::from_secs(30))
                    .map(|(path, _)| path.clone())
            })
            .flatten();
        if audit.is_some() {
            self.last_audit = Instant::now();
        }
        for path in present {
            if audit.as_ref() != Some(&path)
                && !self.pending.contains_key(&path)
                && let Some(seen) = self.seen.get(&path)
                && archive::unchanged(&path, &seen.version).context(ArchiveSnafu)?
            {
                continue;
            }
            self.observe(current, &path, &mut probe)?;
        }
        Ok(())
    }

    fn observe(
        &mut self,
        current: Option<GroupId>,
        path: &Path,
        probe: &mut impl FnMut(&Path, &Path) -> archive::Result<archive::SourceFileVersion>,
    ) -> Result<()> {
        let version = match probe(path, &self.hot_folder) {
            Ok(version) => Some(version),
            Err(ArchiveError::Changed) => None,
            Err(ArchiveError::Io { source }) if matches!(source.raw_os_error(), Some(32 | 33)) => {
                None
            }
            Err(error) => return Err(ArchiveSnafu.into_error(error)),
        };
        if let Some(version) = &version
            && self
                .seen
                .get(path)
                .is_some_and(|seen| &seen.version == version)
        {
            self.checked.insert(path.to_owned(), Instant::now());
            self.pending.remove(path);
            return Ok(());
        }
        if !self.pending.contains_key(path) {
            let crosses_group = self
                .seen
                .get(path)
                .is_some_and(|seen| seen.owner != current);
            let owner = if self.uncertain || crosses_group || self.lost.contains(path) {
                None
            } else {
                current
            };
            self.pending.insert(
                path.to_owned(),
                Pending {
                    owner,
                    version: version.clone(),
                    stable_since: Instant::now(),
                },
            );
            return Ok(());
        }
        let pending = self.pending.get_mut(path).context(InvariantSnafu)?;
        if pending.version != version || version.is_none() {
            pending.version = version;
            pending.stable_since = Instant::now();
            return Ok(());
        }
        if pending.stable_since.elapsed() < INTERVAL {
            return Ok(());
        }
        let version = version.ok_or_else(|| ArchiveSnafu.into_error(ArchiveError::Changed))?;
        let destination = match archive::copy(path, &self.hot_folder, &self.archive, &version) {
            Ok(path) => path,
            Err(ArchiveError::Changed) => {
                pending.version = None;
                return Ok(());
            }
            Err(error) => return Err(ArchiveSnafu.into_error(error)),
        };
        let pending = self.pending.remove(path).context(InvariantSnafu)?;
        self.seen.insert(
            path.to_owned(),
            ObservedSource {
                version: version.clone(),
                owner: pending.owner,
            },
        );
        self.checked.insert(path.to_owned(), Instant::now());
        self.missing_sources
            .retain(|missing| missing.source != path);
        self.photos.push(ArchivedPhoto {
            id: Uuid::now_v7(),
            source: path.to_owned(),
            path: destination,
            sha256: version.sha256,
            bytes: version.bytes,
            modified: version.modified,
            group_id: pending.owner,
        });
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/hot_folder.rs"]
mod tests;
