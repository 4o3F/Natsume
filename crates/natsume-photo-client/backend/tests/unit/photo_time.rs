use super::*;
use chrono::{Timelike as _, Utc};
use snafu::{OptionExt as _, ResultExt as _, Snafu};

#[derive(Debug, Snafu)]
enum TestError {
    Exif { source: exif::Error },
    Image { source: image::ImageError },
    Io { source: std::io::Error },
    Fixture,
}
type TestResult<T = ()> = Result<T, TestError>;

fn metadata(values: &[(Tag, &[u8])]) -> TestResult<Vec<u8>> {
    let fields: Vec<_> = values
        .iter()
        .map(|(tag, bytes)| exif::Field {
            tag: *tag,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![bytes.to_vec()]),
        })
        .collect();
    let mut writer = exif::experimental::Writer::new();
    for field in &fields {
        writer.push_field(field);
    }
    let mut bytes = std::io::Cursor::new(Vec::new());
    writer.write(&mut bytes, true).context(ExifSnafu)?;
    Ok(bytes.into_inner())
}

#[test]
fn original_time_honors_offset_and_fraction_without_using_completion_time() -> TestResult {
    let exif = metadata(&[
        (Tag::DateTimeOriginal, b"2026:10:31 12:34:56"),
        (Tag::OffsetTimeOriginal, b"+08:00"),
        (Tag::SubSecTimeOriginal, b"123456789987"),
    ])?;
    let original = UNIX_EPOCH + Duration::from_secs(7);
    let time = PhotoTime::read(Some(exif), original);
    let expected = Utc
        .with_ymd_and_hms(2026, 10, 31, 4, 34, 56)
        .single()
        .and_then(|time| time.with_nanosecond(123_456_789))
        .context(FixtureSnafu)?;
    assert_eq!(time.instant(), SystemTime::from(expected));
    assert_eq!(time.source(), PhotoTimeSource::ExifOriginal);
    Ok(())
}

#[test]
fn jpeg_container_time_is_retained_before_source_metadata_is_stripped() -> TestResult {
    use crate::capture_session::{ArchivedPhoto, PhotoCheck};
    use image::ImageEncoder as _;
    use sha2::{Digest as _, Sha256};
    let date = metadata(&[
        (Tag::DateTimeOriginal, b"2026:10:31 12:34:56"),
        (Tag::OffsetTimeOriginal, b"+08:00"),
    ])?;
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 92);
    encoder
        .set_exif_metadata(date)
        .map_err(image::ImageError::Unsupported)
        .context(ImageSnafu)?;
    encoder
        .encode_image(&image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 3)))
        .context(ImageSnafu)?;
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let archive = std::fs::canonicalize(directory.path()).context(IoSnafu)?;
    let path = archive.join("image.source");
    std::fs::write(&path, &bytes).context(IoSnafu)?;
    let photo = ArchivedPhoto {
        id: uuid::Uuid::now_v7(),
        source: archive.join("camera.jpg"),
        path,
        sha256: hex::encode(Sha256::digest(&bytes)),
        bytes: bytes.len() as u64,
        modified: UNIX_EPOCH,
        group_id: None,
    };
    let check = PhotoCheck::inspect(&photo, &archive);
    let candidate = check.result().ok().context(FixtureSnafu)?;
    assert_eq!(
        candidate.captured_at().source(),
        PhotoTimeSource::ExifOriginal
    );
    assert_eq!(
        candidate.captured_at().instant(),
        SystemTime::from(
            Utc.with_ymd_and_hms(2026, 10, 31, 4, 34, 56)
                .single()
                .context(FixtureSnafu)?
        )
    );
    Ok(())
}

#[test]
fn missing_offset_is_interpreted_in_the_workstation_local_zone() -> TestResult {
    let exif = metadata(&[(Tag::DateTimeOriginal, b"2026:10:31 13:14:15")])?;
    let fallback = UNIX_EPOCH + Duration::from_secs(42);
    let actual = PhotoTime::read(Some(exif), fallback);
    if let Some(local) = Local.with_ymd_and_hms(2026, 10, 31, 13, 14, 15).single() {
        assert_eq!(actual.instant(), SystemTime::from(local));
        assert_eq!(actual.source(), PhotoTimeSource::ExifOriginal);
    } else {
        assert_eq!(actual.instant(), fallback);
        assert_eq!(actual.source(), PhotoTimeSource::FileModified);
    }
    Ok(())
}

#[test]
fn missing_invalid_and_oversized_exif_fall_back_to_the_original_file_time() -> TestResult {
    let fallback = UNIX_EPOCH
        .checked_sub(Duration::from_secs(1))
        .context(FixtureSnafu)?;
    let cases = [
        None,
        Some(b"not TIFF".to_vec()),
        Some(vec![0; MAX_EXIF_BYTES + 1]),
        Some(metadata(&[(Tag::DateTime, b"2026:10:31 12:34:56")])?),
        Some(metadata(&[(
            Tag::DateTimeOriginal,
            b"2026:02:30 12:34:56",
        )])?),
        Some(metadata(&[(
            Tag::DateTimeOriginal,
            b"2026:10:31 25:34:56",
        )])?),
        Some(metadata(&[
            (Tag::DateTimeOriginal, b"2026:10:31 12:34:56"),
            (Tag::OffsetTimeOriginal, b"+99:00"),
        ])?),
        Some(metadata(&[
            (Tag::DateTimeOriginal, b"2026:10:31 12:34:56"),
            (Tag::SubSecTimeOriginal, b"abc"),
        ])?),
    ];
    for input in cases {
        let actual = PhotoTime::read(input, fallback);
        assert_eq!(actual.instant(), fallback);
        assert_eq!(actual.source(), PhotoTimeSource::FileModified);
    }
    Ok(())
}
