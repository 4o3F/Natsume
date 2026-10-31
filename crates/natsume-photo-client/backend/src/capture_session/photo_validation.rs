//! Bounded source validation and single-decode candidate preparation.
use super::candidate::{self, CandidateError, DecodedPhoto, PhotoCandidate};
use super::{ArchivedPhoto, PhotoTime};
use image::{ImageDecoder as _, ImageFormat, ImageReader, Limits};
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{
    fs::{self, OpenOptions},
    io::{Cursor, Read as _},
    os::windows::fs::OpenOptionsExt as _,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

/// Photography input limit, independent of the smaller initialization-logo budget.
const MAX_BYTES: u64 = 50 * 1024 * 1024;
/// Maximum source pixel count; inspect before allocating the decoded image.
const MAX_PIXELS: u64 = 80_000_000;
/// Decoder/output budget, not a process-wide RSS bound.
const MAX_ALLOCATION: u64 = 256 * 1024 * 1024;

/// Failure to validate one archived photo, independent of directory monitoring.
#[derive(Debug, Snafu)]
pub enum PhotoValidationError {
    #[snafu(display("archived photo file operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("archived photo is not a regular file in the capture directory"))]
    Path,
    #[snafu(display("archived photo exceeds the 50 MiB byte limit"))]
    ByteLimit,
    #[snafu(display("archived photo does not match its recorded bytes or SHA-256"))]
    Integrity,
    #[snafu(display("archived photo is not a JPEG or PNG image"))]
    Format,
    #[snafu(display("archived photo is missing its required image end marker"))]
    Incomplete,
    #[snafu(display("archived photo exceeds the pixel or decoded byte limit"))]
    Dimensions,
    #[snafu(display("archived photo cannot be completely decoded within its allocation limit"))]
    Decode { source: image::ImageError },
    #[snafu(display("candidate preparation failed"))]
    Candidate { source: CandidateError },
}

use crate::photo::{PhotoDimensions, PhotoFormat};

/// A small candidate result; snapshots never retain full-resolution pixel buffers.
#[derive(Debug, Clone)]
pub struct PhotoCheck {
    photo_id: Uuid,
    result: Arc<Result<PhotoCandidate, PhotoValidationError>>,
    checked_at: Instant,
}
impl PhotoCheck {
    /// Identifies the archived copy this result belongs to.
    #[must_use]
    pub fn photo_id(&self) -> Uuid {
        self.photo_id
    }
    /// Reads the prepared candidate or the original typed processing error.
    /// # Errors
    /// Returns the file, integrity, format or decode failure for this copy.
    pub fn result(&self) -> Result<&PhotoCandidate, &PhotoValidationError> {
        self.result.as_ref().as_ref()
    }
    pub(super) fn inspect(photo: &ArchivedPhoto, archive: &Path) -> Self {
        Self {
            photo_id: photo.id,
            result: Arc::new(validate(photo, archive)),
            checked_at: Instant::now(),
        }
    }
    pub(super) fn retryable(&self) -> bool {
        matches!(self.result(), Err(PhotoValidationError::Io { source }
            | PhotoValidationError::Candidate { source: CandidateError::Io { source } })
            if matches!(source.raw_os_error(), Some(32 | 33))
                || matches!(source.kind(), std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
    }
    pub(super) fn retry_due(&self) -> bool {
        self.retryable() && self.checked_at.elapsed() >= Duration::from_secs(1)
    }
}

fn validate(photo: &ArchivedPhoto, archive: &Path) -> Result<PhotoCandidate, PhotoValidationError> {
    ensure!(photo.bytes <= MAX_BYTES, ByteLimitSnafu);
    let metadata = fs::symlink_metadata(&photo.path).context(IoSnafu)?;
    ensure!(metadata.is_file() && !metadata.is_symlink(), PathSnafu);
    let path = fs::canonicalize(&photo.path).context(IoSnafu)?;
    ensure!(path.parent() == Some(archive), PathSnafu);
    // FILE_SHARE_READ excludes concurrent writes and replacement while reading
    // the bounded byte snapshot; decoding only uses those owned bytes.
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .context(IoSnafu)?;
    ensure!(
        file.metadata().context(IoSnafu)?.len() <= MAX_BYTES,
        ByteLimitSnafu
    );
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu)?;
    ensure!(bytes.len() as u64 <= MAX_BYTES, ByteLimitSnafu);
    ensure!(
        bytes.len() as u64 == photo.bytes && hex::encode(Sha256::digest(&bytes)) == photo.sha256,
        IntegritySnafu
    );
    decode(&bytes, photo, archive)
}

fn decode(
    bytes: &[u8],
    photo: &ArchivedPhoto,
    archive: &Path,
) -> Result<PhotoCandidate, PhotoValidationError> {
    let (format, encoded, end_marker) = match image::guess_format(bytes) {
        Ok(ImageFormat::Jpeg) => (PhotoFormat::Jpeg, ImageFormat::Jpeg, b"\xff\xd9".as_slice()),
        Ok(ImageFormat::Png) => (
            PhotoFormat::Png,
            ImageFormat::Png,
            b"\0\0\0\0IEND\xaeB`\x82".as_slice(),
        ),
        _ => return FormatSnafu.fail(),
    };
    // The image decoders may tolerate missing EOI/IEND after pixel data.
    // Capture input must contain the complete encoded file, not just enough pixels.
    ensure!(bytes.ends_with(end_marker), IncompleteSnafu);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), encoded);
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_ALLOCATION);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().context(DecodeSnafu)?;
    let (width, height) = decoder.dimensions();
    ensure!(
        u64::from(width) * u64::from(height) <= MAX_PIXELS
            && decoder.total_bytes() <= MAX_ALLOCATION,
        DimensionsSnafu
    );
    if format == PhotoFormat::Png {
        candidate::check_png_color(bytes).context(CandidateSnafu)?;
    }
    let orientation = decoder.orientation().context(DecodeSnafu)?;
    let captured_at = PhotoTime::read(
        decoder.exif_metadata().context(DecodeSnafu)?,
        photo.modified,
    );
    let icc = decoder.icc_profile().context(DecodeSnafu)?;
    // Decode exactly once, then consume the same pixels for normalized output
    // and its thumbnail on the capture worker's existing blocking task.
    let image = image::DynamicImage::from_decoder(decoder).context(DecodeSnafu)?;
    candidate::prepare(
        photo,
        archive,
        DecodedPhoto {
            dimensions: PhotoDimensions {
                width,
                height,
                format,
            },
            image,
            orientation,
            icc,
            captured_at,
        },
    )
    .context(CandidateSnafu)
}

#[cfg(test)]
#[path = "../../tests/unit/photo_validation.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/unit/candidate.rs"]
mod candidate_tests;
