use super::*;
use snafu::{OptionExt as _, Snafu};

#[derive(Debug, Snafu)]
enum TestError {
    Io { source: std::io::Error },
    Image { source: image::ImageError },
    Fixture,
}
type TestResult<T = ()> = Result<T, TestError>;

fn encoded(format: ImageFormat) -> TestResult<Vec<u8>> {
    let mut output = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 6, image::Rgb([20, 40, 60])))
        .write_to(&mut output, format)
        .context(ImageSnafu)?;
    Ok(output.into_inner())
}

fn archive(bytes: &[u8]) -> TestResult<(tempfile::TempDir, ArchivedPhoto)> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let path = directory.path().join("opaque.source");
    fs::write(&path, bytes).context(IoSnafu)?;
    let path = fs::canonicalize(path).context(IoSnafu)?;
    let photo = ArchivedPhoto {
        id: Uuid::now_v7(),
        source: directory.path().join("camera.jpg"),
        path,
        sha256: hex::encode(Sha256::digest(bytes)),
        bytes: bytes.len() as u64,
        modified: std::time::UNIX_EPOCH,
        group_id: None,
    };
    Ok((directory, photo))
}

fn inspect(photo: &ArchivedPhoto) -> TestResult<PhotoCheck> {
    Ok(PhotoCheck::inspect(
        photo,
        photo.path.parent().context(FixtureSnafu)?,
    ))
}

#[test]
fn decodes_real_jpeg_and_png_from_opaque_archive_names_without_modifying_them() -> TestResult {
    for (encoded_format, expected) in [
        (ImageFormat::Png, PhotoFormat::Png),
        (ImageFormat::Jpeg, PhotoFormat::Jpeg),
    ] {
        let bytes = encoded(encoded_format)?;
        let (_directory, photo) = archive(&bytes)?;
        let check = inspect(&photo)?;
        assert_eq!(check.photo_id(), photo.id);
        assert_eq!(
            check.result().map(PhotoCandidate::source_dimensions).ok(),
            Some(PhotoDimensions {
                width: 8,
                height: 6,
                format: expected
            })
        );
        assert!(!check.retryable());
        assert_eq!(fs::read(&photo.path).context(IoSnafu)?, bytes);
    }
    Ok(())
}

#[test]
fn distinguishes_integrity_format_and_complete_decode_failures() -> TestResult {
    let bytes = encoded(ImageFormat::Png)?;
    let (_directory, mut photo) = archive(&bytes)?;
    photo.sha256 = "0".repeat(64);
    assert!(matches!(
        inspect(&photo)?.result(),
        Err(PhotoValidationError::Integrity)
    ));
    photo.sha256 = hex::encode(Sha256::digest(&bytes));
    photo.bytes += 1;
    assert!(matches!(
        inspect(&photo)?.result(),
        Err(PhotoValidationError::Integrity)
    ));

    let (_directory, photo) = archive(b"not really a JPEG")?;
    assert!(matches!(
        inspect(&photo)?.result(),
        Err(PhotoValidationError::Format)
    ));
    // The complete IHDR still contains dimensions; only decoding the pixel
    // payload will notice this truncation.
    let mut damaged = bytes[..bytes.len() / 2].to_vec();
    damaged.extend_from_slice(&bytes[bytes.len() - 12..]);
    let (_directory, photo) = archive(&damaged)?;
    assert!(matches!(
        inspect(&photo)?.result(),
        Err(PhotoValidationError::Decode { .. })
    ));
    Ok(())
}

#[test]
fn rejects_images_missing_their_end_marker_even_if_pixels_are_decodable() -> TestResult {
    for (format, missing) in [(ImageFormat::Jpeg, 2), (ImageFormat::Png, 12)] {
        let bytes = encoded(format)?;
        let (_directory, photo) = archive(&bytes[..bytes.len() - missing])?;
        assert!(
            matches!(
                inspect(&photo)?.result(),
                Err(PhotoValidationError::Incomplete)
            ),
            "missing end marker must not be accepted: {format:?}"
        );
    }
    Ok(())
}

#[test]
fn byte_limit_rejects_both_recorded_and_actual_oversized_inputs() -> TestResult {
    let (_directory, mut photo) = archive(&encoded(ImageFormat::Png)?)?;
    photo.bytes = MAX_BYTES + 1;
    assert!(matches!(
        inspect(&photo)?.result(),
        Err(PhotoValidationError::ByteLimit)
    ));
    photo.bytes = 1;
    OpenOptions::new()
        .write(true)
        .open(&photo.path)
        .context(IoSnafu)?
        .set_len(MAX_BYTES + 1)
        .context(IoSnafu)?;
    assert!(matches!(
        inspect(&photo)?.result(),
        Err(PhotoValidationError::ByteLimit)
    ));
    Ok(())
}

#[test]
fn pixel_limit_rejects_a_valid_header_before_large_pixel_allocation() -> TestResult {
    let mut bytes = encoded(ImageFormat::Png)?;
    bytes[16..20].copy_from_slice(&10_000_u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&8_001_u32.to_be_bytes());
    // Recompute the fixture's IHDR CRC so the decoder reaches the pixel limit,
    // rather than rejecting an unrelated checksum error.
    let mut crc = u32::MAX;
    for byte in &bytes[12..29] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    bytes[29..33].copy_from_slice(&(!crc).to_be_bytes());
    let (_directory, photo) = archive(&bytes)?;
    assert!(matches!(
        inspect(&photo)?.result(),
        Err(PhotoValidationError::Dimensions)
    ));
    Ok(())
}

#[test]
fn retries_sharing_conflicts_but_not_missing_or_outside_archives() -> TestResult {
    let (_directory, photo) = archive(&encoded(ImageFormat::Png)?)?;
    let unrelated = tempfile::tempdir().context(IoSnafu)?;
    let check = PhotoCheck::inspect(&photo, unrelated.path());
    assert!(matches!(check.result(), Err(PhotoValidationError::Path)));
    assert!(!check.retryable());
    let locked = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&photo.path)
        .context(IoSnafu)?;
    let mut check = inspect(&photo)?;
    assert!(matches!(
        check.result(),
        Err(PhotoValidationError::Io { .. })
    ));
    assert!(check.retryable());
    assert!(!check.retry_due());
    check.checked_at = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .context(FixtureSnafu)?;
    assert!(check.retry_due());
    drop(locked);
    assert!(inspect(&photo)?.result().is_ok());
    fs::remove_file(&photo.path).context(IoSnafu)?;
    let check = inspect(&photo)?;
    assert!(matches!(
        check.result(),
        Err(PhotoValidationError::Io { .. })
    ));
    assert!(
        !check.retryable(),
        "a vanished archive must not pin the group forever"
    );
    Ok(())
}
