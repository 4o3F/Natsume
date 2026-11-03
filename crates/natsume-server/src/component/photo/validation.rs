//! Durable package verification, owned by the Server background task group.
mod archive;
mod directory;
mod worker;
pub(super) use worker::trusted;

use super::{db, results_storage};
use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub(crate) enum ValidationError {
    #[snafu(display("Photo archive structure or resource limit is invalid"))]
    Archive,
    #[snafu(display("Photo ZIP decoding failed"))]
    Zip { source: zip::result::ZipError },
    #[snafu(display("Photo manifest JSON is invalid"))]
    Json { source: serde_json::Error },
    #[snafu(display("Photo stored initialization JSON is invalid"))]
    SnapshotJson { source: serde_json::Error },
    #[snafu(display("Photo stored initialization contract is invalid"))]
    SnapshotContract {
        source: natsume_photo_protocol::ValidationError,
    },
    #[snafu(display("Photo result signature or contract is invalid"))]
    Signature {
        source: natsume_photo_protocol::results::ResultError,
    },
    #[snafu(display("Photo source is not authorized for this initialization"))]
    Authorization,
    #[snafu(display("Photo image bytes or dimensions disagree"))]
    Integrity,
    #[snafu(display("Photo image decoding failed"))]
    Image { source: image::ImageError },
    #[snafu(display("Photo image allocation is unavailable"))]
    Allocation {
        source: std::collections::TryReserveError,
    },
    #[snafu(display("Photo manifest identity was reused with different content"))]
    Conflict,
    #[snafu(display("Photo validation was interrupted"))]
    Interrupted,
    #[snafu(display("Photo validation storage failed"))]
    Storage {
        source: results_storage::ResultsStorageError,
    },
    #[snafu(display("Photo validation filesystem operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("Photo validation persistence failed"))]
    Persistence { source: db::PersistenceError },
    #[snafu(display("Photo validation worker failed"))]
    Worker {
        #[snafu(source(from(tokio::task::JoinError, crate::task::TaskFailure::from)))]
        source: crate::task::TaskFailure,
    },
    #[snafu(display("Photo initialization records are inconsistent"))]
    Corrupt,
}

impl From<crate::db::PersistenceError> for ValidationError {
    fn from(source: crate::db::PersistenceError) -> Self {
        Self::Persistence {
            source: source.into(),
        }
    }
}

type Result<T> = std::result::Result<T, ValidationError>;
