//! Shared local photo values, independent of capture policy and persistence.
use natsume_photo_protocol::Identity;
pub use natsume_photo_protocol::results::{PhotoFormat, ResultId, SubmissionId};
/// Dimensions and encoding; the owning value distinguishes source from normalized output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhotoDimensions {
    pub width: u32,
    pub height: u32,
    pub format: PhotoFormat,
}

/// Local identity role for one exact confirmation attempt across retries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfirmationRole {}
pub type ConfirmationId = Identity<ConfirmationRole>;
