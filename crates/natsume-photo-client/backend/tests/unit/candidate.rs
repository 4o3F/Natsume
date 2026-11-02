use super::*;
use image::{DynamicImage, ImageEncoder as _, metadata::Orientation};
use snafu::{OptionExt as _, Snafu};

#[derive(Debug, Snafu)]
enum TestError {
    Io { source: std::io::Error },
    Image { source: image::ImageError },
    Validation { source: PhotoValidationError },
    Fixture,
}
type TestResult<T = ()> = Result<T, TestError>;

fn archived(directory: &tempfile::TempDir, bytes: &[u8]) -> TestResult<ArchivedPhoto> {
    let archive = fs::canonicalize(directory.path()).context(IoSnafu)?;
    let path = archive.join("original.source");
    fs::write(&path, bytes).context(IoSnafu)?;
    Ok(ArchivedPhoto {
        id: Uuid::now_v7(),
        source: archive.join("camera.png"),
        path,
        sha256: hex::encode(Sha256::digest(bytes)),
        bytes: bytes.len() as u64,
        modified: std::time::UNIX_EPOCH,
        group_id: None,
    })
}

fn process(bytes: &[u8]) -> TestResult<(tempfile::TempDir, PhotoCandidate)> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let photo = archived(&directory, bytes)?;
    let check = PhotoCheck::inspect(&photo, photo.path.parent().context(FixtureSnafu)?);
    let result = Arc::try_unwrap(check.result).ok().context(FixtureSnafu)?;
    Ok((directory, result.context(ValidationSnafu)?))
}

fn exif(orientation: u8) -> Vec<u8> {
    // Little-endian TIFF IFD: one SHORT Orientation, followed by a canary
    // representing private source metadata which must never enter derivatives.
    let mut exif = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
    exif.extend_from_slice(&[orientation, 0, 0, 0, 0, 0, 0, 0]);
    exif.extend_from_slice(b"PRIVATE_GPS_CANARY");
    exif
}

fn png(image: &DynamicImage, orientation: u8, icc: Option<Vec<u8>>) -> TestResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    encoder
        .set_exif_metadata(exif(orientation))
        .map_err(image::ImageError::Unsupported)
        .context(ImageSnafu)?;
    if let Some(icc) = icc {
        encoder
            .set_icc_profile(icc)
            .map_err(image::ImageError::Unsupported)
            .context(ImageSnafu)?;
    }
    encoder
        .write_image(
            image.as_bytes(),
            image.width(),
            image.height(),
            image.color().into(),
        )
        .context(ImageSnafu)?;
    Ok(bytes)
}

#[test]
fn jpeg_derivatives_apply_orientation_and_strip_original_exif() -> TestResult {
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 100);
    encoder
        .set_exif_metadata(exif(6))
        .map_err(image::ImageError::Unsupported)
        .context(ImageSnafu)?;
    encoder
        .encode_image(&DynamicImage::ImageRgb8(image::RgbImage::new(8, 6)))
        .context(ImageSnafu)?;
    let (_directory, candidate) = process(&bytes)?;
    assert_eq!(
        candidate.image().dimensions(),
        PhotoDimensions {
            width: 6,
            height: 8,
            format: PhotoFormat::Jpeg
        }
    );
    let output = fs::read(candidate.image().path()).context(IoSnafu)?;
    assert!(output.starts_with(b"\xff\xd8"));
    let mut decoder = ImageReader::new(Cursor::new(&output))
        .with_guessed_format()
        .context(IoSnafu)?
        .into_decoder()
        .context(ImageSnafu)?;
    assert!(decoder.exif_metadata().context(ImageSnafu)?.is_none());
    assert!(decoder.icc_profile().context(ImageSnafu)?.is_none());
    Ok(())
}

#[test]
fn applies_all_exif_orientations_and_removes_source_metadata() -> TestResult {
    let original = DynamicImage::ImageRgb8(
        image::RgbImage::from_raw(
            2,
            3,
            vec![1, 0, 0, 2, 0, 0, 3, 0, 0, 4, 0, 0, 5, 0, 0, 6, 0, 0],
        )
        .context(FixtureSnafu)?,
    );
    for (orientation, width, expected) in [
        (1, 2, [1, 2, 3, 4, 5, 6]),
        (2, 2, [2, 1, 4, 3, 6, 5]),
        (3, 2, [6, 5, 4, 3, 2, 1]),
        (4, 2, [5, 6, 3, 4, 1, 2]),
        (5, 3, [1, 3, 5, 2, 4, 6]),
        (6, 3, [5, 3, 1, 6, 4, 2]),
        (7, 3, [6, 4, 2, 5, 3, 1]),
        (8, 3, [2, 4, 6, 1, 3, 5]),
    ] {
        let bytes = png(&original, orientation, None)?;
        let (directory, candidate) = process(&bytes)?;
        assert_eq!(
            fs::read(directory.path().join("original.source")).context(IoSnafu)?,
            bytes
        );
        assert_eq!(candidate.image().dimensions().width, width);
        let normalized = image::open(candidate.image().path())
            .context(ImageSnafu)?
            .into_rgb8();
        assert_eq!(
            normalized
                .pixels()
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            expected
        );
        for file in [candidate.image(), candidate.thumbnail()] {
            let bytes = fs::read(file.path()).context(IoSnafu)?;
            assert_eq!(file.bytes(), bytes.len() as u64);
            assert_eq!(file.sha256(), hex::encode(Sha256::digest(&bytes)));
            assert!(
                !bytes
                    .windows(b"PRIVATE_GPS_CANARY".len())
                    .any(|bytes| bytes == b"PRIVATE_GPS_CANARY")
            );
            let mut decoder = ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .context(IoSnafu)?
                .into_decoder()
                .context(ImageSnafu)?;
            assert_eq!(
                decoder.orientation().context(ImageSnafu)?,
                Orientation::NoTransforms
            );
            assert!(decoder.exif_metadata().context(ImageSnafu)?.is_none());
            assert!(decoder.icc_profile().context(ImageSnafu)?.is_none());
        }
    }
    Ok(())
}

#[test]
fn thumbnails_preserve_alpha_and_aspect_ratio_without_upscaling_small_inputs() -> TestResult {
    for (width, height, expected) in [(1200, 600, (512, 256)), (8, 6, (8, 6))] {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([80, 90, 100, 123]),
        ));
        let (_directory, candidate) = process(&png(&image, 1, None)?)?;
        let normalized = image::open(candidate.image().path())
            .context(ImageSnafu)?
            .into_rgba8();
        assert_eq!(normalized.dimensions(), (width, height));
        assert_eq!(normalized.get_pixel(0, 0)[3], 123);
        let thumbnail = image::open(candidate.thumbnail().path())
            .context(ImageSnafu)?
            .into_rgba8();
        assert_eq!(thumbnail.dimensions(), expected);
        assert_eq!(thumbnail.get_pixel(0, 0)[3], 123);
    }
    Ok(())
}

#[test]
fn publication_failure_preserves_existing_files_and_removes_private_staging() -> TestResult {
    use crate::capture_session::CandidateError;
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let image = DynamicImage::ImageRgb8(image::RgbImage::new(2, 2));
    let bytes = png(&image, 1, None)?;
    let photo = archived(&directory, &bytes)?;
    let target = directory.path().join(format!("{}.candidate", photo.id));
    fs::create_dir(&target).context(IoSnafu)?;
    let sentinel = target.join("keep.txt");
    fs::write(&sentinel, b"existing data").context(IoSnafu)?;
    let check = PhotoCheck::inspect(&photo, photo.path.parent().context(FixtureSnafu)?);
    assert!(matches!(
        check.result(),
        Err(PhotoValidationError::Candidate {
            source: CandidateError::Io { .. }
        })
    ));
    assert!(!check.retryable());
    assert_eq!(fs::read(&sentinel).context(IoSnafu)?, b"existing data");
    assert_eq!(fs::read(&photo.path).context(IoSnafu)?, bytes);
    assert_eq!(fs::read_dir(directory.path()).context(IoSnafu)?.count(), 2);
    assert_eq!(fs::read_dir(&target).context(IoSnafu)?.count(), 1);
    Ok(())
}
