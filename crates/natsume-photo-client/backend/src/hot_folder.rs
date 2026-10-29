//! Tokio-owned ingestion state, safe native notifications and copy-only archives.
use crate::capture::{self, GroupId};
use snafu::Snafu;
use std::{path::PathBuf, time::Duration};
use uuid::Uuid;

pub(crate) mod archive;
pub(crate) mod native;
pub use archive::ArchiveError;

pub(crate) const INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum Error {
    #[snafu(display("hot-folder directory operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("native directory monitoring failed"))]
    Monitor { source: windows::core::Error },
    #[snafu(display("cannot read the workspace roster: {source}"))]
    Workspace { source: crate::workspace::Error },
    #[snafu(display("capture transition rejected: {source}"))]
    Capture { source: capture::Error },
    #[snafu(display("source archival failed: {source}"))]
    Archive { source: ArchiveError },
    #[snafu(display("hot folder must be an absolute local directory outside the workspace"))]
    UnsafePath,
    #[snafu(display("a new capture session requires an empty hot folder"))]
    NotEmpty,
    #[snafu(display("directory monitoring has not recovered; group transitions are blocked"))]
    MonitorUnavailable,
    #[snafu(display("photo cannot be assigned to this active group"))]
    NotReviewable,
    #[snafu(display("ingestion state invariant was violated"))]
    Invariant,
    #[snafu(display("ingestion worker has stopped"))]
    Stopped,
    #[snafu(display("ingestion task failed"))]
    Worker { source: tokio::task::JoinError },
}
pub type Result<T> = std::result::Result<T, Error>;

/// Persistent-in-memory worker state, distinct from a failed method call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestionState {
    Running,
    RecoveringMonitor,
    PausedDirectory,
    PausedArchive,
    Stopped,
}

/// A stable source copy, never a decoded candidate or a confirmed final.
#[derive(Debug, Clone)]
pub struct ArchivedPhoto {
    pub id: Uuid,
    pub source: PathBuf,
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    /// None requires explicit photographer review.
    pub group_id: Option<GroupId>,
}
/// A source that vanished before archival, retained for photographer review.
#[derive(Debug, Clone)]
pub struct MissingSource {
    pub source: PathBuf,
    pub group_id: Option<GroupId>,
}
/// Owned operator facts; this is not the contestant presentation state.
#[derive(Debug, Clone)]
pub struct HotFolderSnapshot {
    pub session_id: Uuid,
    pub group_id: Option<GroupId>,
    pub account_id: Option<String>,
    pub pending_tasks: usize,
    pub paused: bool,
    pub state: IngestionState,
    pub photos: Vec<ArchivedPhoto>,
    pub missing_sources: Vec<MissingSource>,
    pub notifications: u64,
}
