//! Oriented image derivatives prepared with image from a single bounded decode.
use super::{ArchivedPhoto, PhotoDimensions, PhotoFormat, PhotoTime};
use image::{DynamicImage, ImageEncoder as _, metadata::Orientation};
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// Derivatives remain compatible with the result-package single-image limit.
const OUTPUT_LIMIT: u64 = 50 * 1024 * 1024;
/// Bound normalized pixel buffers as well as the source decoder's output.
const PIXEL_BYTE_LIMIT: u64 = 256 * 1024 * 1024;
const THUMBNAIL_EDGE: u32 = 512;
const JPEG_QUALITY: u8 = 92;

/// Derivative creation failure, separate from source integrity and decoding.
#[derive(Debug, Snafu)]
pub enum CandidateError {
    #[snafu(display("candidate file operation failed"))]
    Io { source: io::Error },
    #[snafu(display("candidate encoding failed or exceeded its output limit"))]
    Encode { source: image::ImageError },
    #[snafu(display("candidate normalized pixel buffer exceeds its size limit"))]
    PixelLimit,
}
type Result<T> = std::result::Result<T, CandidateError>;

/// One immutable image file published inside this capture session's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateImage {
    path: PathBuf,
    sha256: String,
    bytes: u64,
    dimensions: PhotoDimensions,
}
impl CandidateImage {
    /// Holds a read handle that prevents cleanup or modification until dropped.
    /// Open this before handing bytes to a preview renderer; cloning a path is not a lease.
    /// # Errors
    /// Reports a missing image, sharing violation or other file error.
    pub fn open(&self) -> Result<File> {
        use std::os::windows::fs::OpenOptionsExt as _;
        fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&self.path)
            .context(IoSnafu)
    }
    /// Returns the immutable local file used by the future presentation adapter.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Returns the encoded file's lowercase SHA-256.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    /// Returns the encoded file length, excluding other derivatives.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    /// Returns the output dimensions and encoding, after orientation.
    #[must_use]
    pub fn dimensions(&self) -> PhotoDimensions {
        self.dimensions
    }
}

/// A prepared candidate, not a selected image or a confirmed team result.
#[derive(Debug, Clone)]
pub struct PhotoCandidate {
    photo_id: Uuid,
    source_dimensions: PhotoDimensions,
    image: CandidateImage,
    thumbnail: CandidateImage,
    captured_at: PhotoTime,
}
impl PhotoCandidate {
    /// Identifies the archived observation from which this candidate was prepared.
    #[must_use]
    pub fn photo_id(&self) -> Uuid {
        self.photo_id
    }
    /// Returns the encoded source dimensions, before applying EXIF orientation.
    #[must_use]
    pub fn source_dimensions(&self) -> PhotoDimensions {
        self.source_dimensions
    }
    /// Returns the normalized full-resolution candidate file.
    #[must_use]
    pub fn image(&self) -> &CandidateImage {
        &self.image
    }
    /// Returns its small PNG preview file.
    #[must_use]
    pub fn thumbnail(&self) -> &CandidateImage {
        &self.thumbnail
    }
    /// Returns the original source file modification time.
    #[must_use]
    pub fn captured_at(&self) -> PhotoTime {
        self.captured_at
    }
}

pub(super) struct DecodedPhoto {
    pub dimensions: PhotoDimensions,
    pub image: DynamicImage,
    pub orientation: Orientation,
    pub captured_at: PhotoTime,
}

pub(super) fn prepare(
    photo: &ArchivedPhoto,
    archive: &Path,
    decoded: DecodedPhoto,
) -> Result<PhotoCandidate> {
    let mut image = normalize(decoded.image)?;
    image.apply_orientation(decoded.orientation);
    let staging = tempfile::Builder::new()
        .prefix(".candidate-")
        .tempdir_in(archive)
        .context(IoSnafu)?;
    let extension = match decoded.dimensions.format {
        PhotoFormat::Jpeg => "jpg",
        PhotoFormat::Png => "png",
    };
    let filename = format!("image.{extension}");
    let mut full = encode(
        &image,
        decoded.dimensions.format,
        &staging.path().join(&filename),
    )?;
    let thumbnail = image.thumbnail(
        image.width().min(THUMBNAIL_EDGE),
        image.height().min(THUMBNAIL_EDGE),
    );
    // Release the full-resolution buffer before encoding the small derivative.
    drop(image);
    let mut small = encode(
        &thumbnail,
        PhotoFormat::Png,
        &staging.path().join("thumbnail.png"),
    )?;
    let target = archive.join(format!("{}.candidate", photo.id));
    // On Windows a directory rename refuses an existing destination directory.
    // Publish both complete, synced files together; no partial candidate is exposed.
    fs::rename(staging.path(), &target).context(IoSnafu)?;
    let _published = staging.keep();
    full.path = target.join(filename);
    small.path = target.join("thumbnail.png");
    Ok(PhotoCandidate {
        photo_id: photo.id,
        source_dimensions: decoded.dimensions,
        image: full,
        thumbnail: small,
        captured_at: decoded.captured_at,
    })
}

fn normalize(image: DynamicImage) -> Result<DynamicImage> {
    let alpha = image.color().has_alpha();
    let channels = if alpha { 4_u64 } else { 3 };
    ensure!(
        u64::from(image.width()) * u64::from(image.height()) * channels <= PIXEL_BYTE_LIMIT,
        PixelLimitSnafu
    );
    Ok(if alpha {
        DynamicImage::ImageRgba8(image.into_rgba8())
    } else {
        DynamicImage::ImageRgb8(image.into_rgb8())
    })
}

struct CandidateWriter {
    file: BufWriter<File>,
    hash: Sha256,
    bytes: u64,
}
impl Write for CandidateWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > OUTPUT_LIMIT - self.bytes {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        let count = self.file.write(bytes)?;
        self.hash.update(&bytes[..count]);
        self.bytes += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn encode(image: &DynamicImage, format: PhotoFormat, path: &Path) -> Result<CandidateImage> {
    let file = File::create_new(path).context(IoSnafu)?;
    let mut writer = CandidateWriter {
        file: BufWriter::new(file),
        hash: Sha256::new(),
        bytes: 0,
    };
    // Fresh encoders retain no camera EXIF/GPS/XMP or ICC metadata.
    match format {
        PhotoFormat::Jpeg => {
            let mut encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, JPEG_QUALITY);
            encoder.encode_image(image).context(EncodeSnafu)?;
        }
        PhotoFormat::Png => {
            let encoder = image::codecs::png::PngEncoder::new(&mut writer);
            encoder
                .write_image(
                    image.as_bytes(),
                    image.width(),
                    image.height(),
                    image.color().into(),
                )
                .context(EncodeSnafu)?;
        }
    }
    writer.flush().context(IoSnafu)?;
    writer.file.get_ref().sync_all().context(IoSnafu)?;
    Ok(CandidateImage {
        path: path.to_owned(),
        sha256: hex::encode(writer.hash.finalize()),
        bytes: writer.bytes,
        dimensions: PhotoDimensions {
            width: image.width(),
            height: image.height(),
            format,
        },
    })
}

#[cfg(test)]
#[path = "../../tests/unit/candidate_output.rs"]
mod tests;
