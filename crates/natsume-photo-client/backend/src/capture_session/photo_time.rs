//! Candidate ordering uses the observed source file modification time.
use std::time::SystemTime;
/// Provenance of the timestamp used for candidate ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoTimeSource {
    FileModified,
}
/// Original source time, independent of copying and processing completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhotoTime {
    instant: SystemTime,
}
impl PhotoTime {
    /// Returns the original file modification time.
    #[must_use]
    pub fn instant(self) -> SystemTime {
        self.instant
    }
    /// Returns the timestamp's filesystem provenance.
    #[must_use]
    pub fn source(self) -> PhotoTimeSource {
        PhotoTimeSource::FileModified
    }
    pub(super) fn read(modified: SystemTime) -> Self {
        Self { instant: modified }
    }
}
#[cfg(test)]
#[path = "../../tests/unit/photo_time.rs"]
mod tests;
