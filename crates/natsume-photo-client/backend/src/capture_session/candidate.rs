//! Oriented, sRGB image derivatives prepared from a single bounded decode.
use super::{ArchivedPhoto, PhotoDimensions, PhotoFormat, PhotoTime};
use image::{DynamicImage, ImageEncoder as _, metadata::Orientation};
use moxcms::{ColorProfile, DataColorSpace, Layout, ParsingOptions, TransformOptions};
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// Bound profile parsing independently of the compressed photo's 50 MiB limit.
const PROFILE_LIMIT: usize = 4 * 1024 * 1024;
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
    #[snafu(display("candidate ICC profile or color conversion failed"))]
    Color { source: moxcms::CmsError },
    #[snafu(display("candidate ICC profile exceeds its size limit"))]
    ProfileLimit,
    #[snafu(display("candidate color declaration is not supported"))]
    UnsupportedColor,
    #[snafu(display("candidate normalized pixel buffer exceeds its size limit"))]
    PixelLimit,
    #[snafu(display("PNG color metadata cannot be read within its limit"))]
    PngMetadata { source: png::DecodingError },
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
    /// Returns camera time or the original source modification-time fallback.
    #[must_use]
    pub fn captured_at(&self) -> PhotoTime {
        self.captured_at
    }
}

pub(super) struct DecodedPhoto {
    pub dimensions: PhotoDimensions,
    pub image: DynamicImage,
    pub orientation: Orientation,
    pub icc: Option<Vec<u8>>,
    pub captured_at: PhotoTime,
}

/// Refuse declarations which cannot be converted by this RGB ICC pipeline.
pub(super) fn check_png_color(bytes: &[u8]) -> Result<()> {
    let mut decoder = png::Decoder::new(io::Cursor::new(bytes));
    decoder.set_limits(png::Limits {
        bytes: PROFILE_LIMIT,
    });
    let reader = decoder.read_info().context(PngMetadataSnafu)?;
    let info = reader.info();
    ensure!(
        info.coding_independent_code_points.is_none()
            && info.mastering_display_color_volume.is_none()
            && info.content_light_level.is_none(),
        UnsupportedColorSnafu
    );
    ensure!(
        info.srgb.is_some()
            || info.icc_profile.is_some()
            || (info.gama_chunk.is_none() && info.chrm_chunk.is_none()),
        UnsupportedColorSnafu
    );
    Ok(())
}

pub(super) fn prepare(
    photo: &ArchivedPhoto,
    archive: &Path,
    decoded: DecodedPhoto,
) -> Result<PhotoCandidate> {
    let mut image = normalize(decoded.image, decoded.icc.as_deref())?;
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

fn normalize(image: DynamicImage, icc: Option<&[u8]>) -> Result<DynamicImage> {
    let alpha = image.color().has_alpha();
    let channels = if alpha { 4_u64 } else { 3 };
    ensure!(
        u64::from(image.width()) * u64::from(image.height()) * channels <= PIXEL_BYTE_LIMIT,
        PixelLimitSnafu
    );
    let profile = if let Some(icc) = icc {
        ensure!(icc.len() <= PROFILE_LIMIT, ProfileLimitSnafu);
        let profile = ColorProfile::new_from_slice_with_options(
            icc,
            ParsingOptions {
                max_profile_size: PROFILE_LIMIT,
                max_allowed_clut_size: PROFILE_LIMIT,
                max_allowed_trc_size: 40_000,
            },
        )
        .context(ColorSnafu)?;
        // The image JPEG decoder already converts CMYK to RGB; do not apply a
        // CMYK/gray ICC transform to buffers whose channels have changed meaning.
        ensure!(
            profile.color_space == DataColorSpace::Rgb && image.color().has_color(),
            UnsupportedColorSnafu
        );
        Some(profile)
    } else {
        None
    };
    if alpha {
        let mut pixels = image.into_rgba8();
        if let Some(profile) = profile {
            convert_rows(
                &profile,
                Layout::Rgba,
                pixels.width() as usize * 4,
                pixels.as_mut(),
            )?;
        }
        Ok(DynamicImage::ImageRgba8(pixels))
    } else {
        let mut pixels = image.into_rgb8();
        if let Some(profile) = profile {
            convert_rows(
                &profile,
                Layout::Rgb,
                pixels.width() as usize * 3,
                pixels.as_mut(),
            )?;
        }
        Ok(DynamicImage::ImageRgb8(pixels))
    }
}

fn convert_rows(
    profile: &ColorProfile,
    layout: Layout,
    row_bytes: usize,
    bytes: &mut [u8],
) -> Result<()> {
    let transform = profile
        .create_transform_8bit(
            layout,
            &ColorProfile::new_srgb(),
            layout,
            TransformOptions::default(),
        )
        .context(ColorSnafu)?;
    let mut row = vec![0; row_bytes];
    for pixels in bytes.chunks_exact_mut(row_bytes) {
        transform.transform(pixels, &mut row).context(ColorSnafu)?;
        pixels.copy_from_slice(&row);
    }
    Ok(())
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
    let profile = output_profile()?;
    // Fresh encoders retain no EXIF/GPS/XMP or original ICC; only the known sRGB profile.
    match format {
        PhotoFormat::Jpeg => {
            let mut encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, JPEG_QUALITY);
            encoder
                .set_icc_profile(profile)
                .map_err(image::ImageError::Unsupported)
                .context(EncodeSnafu)?;
            encoder.encode_image(image).context(EncodeSnafu)?;
        }
        PhotoFormat::Png => {
            let mut encoder = image::codecs::png::PngEncoder::new(&mut writer);
            encoder
                .set_icc_profile(profile)
                .map_err(image::ImageError::Unsupported)
                .context(EncodeSnafu)?;
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

fn output_profile() -> Result<Vec<u8>> {
    let mut profile = ColorProfile::new_srgb().encode().context(ColorSnafu)?;
    // moxcms writes wall-clock time into the ICC header (six big-endian u16s
    // at bytes 24..36). This is our fixed standard profile, not camera metadata.
    // Canonicalize its date so equal normalized pixels have stable file hashes.
    // The encoder always returns a complete ICC header of at least 128 bytes;
    // its profile ID is zero, so no profile-ID digest needs recomputing.
    profile[24..36].copy_from_slice(&[0x07, 0xd0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0]);
    Ok(profile)
}

#[cfg(test)]
#[path = "../../tests/unit/candidate_output.rs"]
mod tests;
