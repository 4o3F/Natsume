//! Optional camera metadata becomes a comparable instant, with a stable source-time fallback.
use chrono::{FixedOffset, Local, NaiveDate, TimeZone as _};
use exif::{In, Tag, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Optional time parsing must not expand an unbounded camera metadata document.
const MAX_EXIF_BYTES: usize = 1024 * 1024;

/// Provenance of the timestamp used for candidate ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoTimeSource {
    ExifOriginal,
    FileModified,
}

/// A normalized instant and its provenance; independent of processing completion time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhotoTime {
    instant: SystemTime,
    source: PhotoTimeSource,
}
impl PhotoTime {
    /// Returns the time used to order this candidate.
    #[must_use]
    pub fn instant(self) -> SystemTime {
        self.instant
    }
    /// Identifies EXIF or the source version's original file modification time.
    #[must_use]
    pub fn source(self) -> PhotoTimeSource {
        self.source
    }
    pub(super) fn read(exif: Option<Vec<u8>>, modified: SystemTime) -> Self {
        if let Some(instant) = exif.and_then(parse) {
            Self {
                instant,
                source: PhotoTimeSource::ExifOriginal,
            }
        } else {
            Self {
                instant: modified,
                source: PhotoTimeSource::FileModified,
            }
        }
    }
}

fn ascii(exif: &exif::Exif, tag: Tag) -> Option<&[u8]> {
    match &exif.get_field(tag, In::PRIMARY)?.value {
        Value::Ascii(values) => values.first().map(Vec::as_slice),
        _ => None,
    }
}

fn parse(bytes: Vec<u8>) -> Option<SystemTime> {
    if bytes.len() > MAX_EXIF_BYTES {
        return None;
    }
    // Missing/malformed optional camera metadata is a specified fallback, not
    // an application failure; never log the source EXIF payload or its values.
    let exif = exif::Reader::new().read_raw(bytes).ok()?;
    let mut value = exif::DateTime::from_ascii(ascii(&exif, Tag::DateTimeOriginal)?).ok()?;
    if exif
        .get_field(Tag::SubSecTimeOriginal, In::PRIMARY)
        .is_some()
    {
        value
            .parse_subsec(ascii(&exif, Tag::SubSecTimeOriginal)?)
            .ok()?;
    }
    if exif
        .get_field(Tag::OffsetTimeOriginal, In::PRIMARY)
        .is_some()
    {
        value
            .parse_offset(ascii(&exif, Tag::OffsetTimeOriginal)?)
            .ok()?;
    }
    // exif::DateTime parses numbers without validating their calendar ranges.
    let naive = NaiveDate::from_ymd_opt(
        i32::from(value.year),
        u32::from(value.month),
        u32::from(value.day),
    )?
    .and_hms_nano_opt(
        u32::from(value.hour),
        u32::from(value.minute),
        u32::from(value.second),
        value.nanosecond.unwrap_or(0),
    )?;
    let (seconds, nanos) = if let Some(offset) = value.offset {
        let time = FixedOffset::east_opt(i32::from(offset) * 60)?
            .from_local_datetime(&naive)
            .single()?;
        (time.timestamp(), time.timestamp_subsec_nanos())
    } else {
        // A camera without an offset is interpreted in the workstation's local
        // timezone. DST gaps or ambiguities do not silently choose an instant.
        let time = Local.from_local_datetime(&naive).single()?;
        (time.timestamp(), time.timestamp_subsec_nanos())
    };
    let whole = Duration::from_secs(seconds.unsigned_abs());
    let instant = if seconds >= 0 {
        UNIX_EPOCH.checked_add(whole)?
    } else {
        UNIX_EPOCH.checked_sub(whole)?
    };
    instant.checked_add(Duration::from_nanos(u64::from(nanos)))
}

#[cfg(test)]
#[path = "../../tests/unit/photo_time.rs"]
mod tests;
