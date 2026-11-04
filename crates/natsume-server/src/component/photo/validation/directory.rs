//! Bound ZIP metadata before the generic ZIP reader allocates its directory.
use super::{ArchiveSnafu, IoSnafu, Result};
use natsume_photo_protocol::results::{PACKAGE_ENTRY_LIMIT, PACKAGE_LIMIT, ZIP_METADATA_LIMIT};
use snafu::{OptionExt as _, ResultExt as _, ensure};
use std::{
    fs::File,
    io::{Read as _, Seek as _, SeekFrom},
};

pub(super) struct Directory {
    pub start: u64,
    pub bytes: u64,
    pub entries: usize,
}

fn number<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    bytes
        .get(offset..offset + N)
        .context(ArchiveSnafu)?
        .try_into()
        .map_err(|_| ArchiveSnafu.build())
}

fn read(file: &mut File, offset: u64, size: usize) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset)).context(IoSnafu)?;
    let mut bytes = vec![0; size];
    file.read_exact(&mut bytes).context(IoSnafu)?;
    Ok(bytes)
}

pub(super) fn preflight(file: &mut File, expected_bytes: u64) -> Result<Directory> {
    let length = file.metadata().context(IoSnafu)?.len();
    ensure!(
        (22..=PACKAGE_LIMIT).contains(&length) && length == expected_bytes,
        ArchiveSnafu
    );
    let tail_size = length.min(super::super::limits::ZIP_EOCD_SCAN_BYTES) as usize;
    let tail_start = length - tail_size as u64;
    let tail = read(file, tail_start, tail_size)?;
    let end = tail
        .windows(4)
        .enumerate()
        .rev()
        .find_map(|(i, bytes)| {
            (bytes == b"PK\x05\x06"
                && i + 22 <= tail.len()
                && i + 22 + usize::from(u16::from_le_bytes([tail[i + 20], tail[i + 21]]))
                    == tail.len())
            .then_some(i)
        })
        .context(ArchiveSnafu)?;
    ensure!(
        u16::from_le_bytes(number(&tail, end + 4)?) == 0
            && u16::from_le_bytes(number(&tail, end + 6)?) == 0,
        ArchiveSnafu
    );
    let count = u16::from_le_bytes(number(&tail, end + 10)?);
    ensure!(
        u16::from_le_bytes(number(&tail, end + 8)?) == count,
        ArchiveSnafu
    );
    let size = u32::from_le_bytes(number(&tail, end + 12)?);
    let start = u32::from_le_bytes(number(&tail, end + 16)?);
    let end_offset = tail_start + end as u64;
    let (start, bytes, count, footer) =
        if count == u16::MAX || size == u32::MAX || start == u32::MAX {
            zip64(file, end_offset)?
        } else {
            (
                u64::from(start),
                u64::from(size),
                u64::from(count),
                end_offset,
            )
        };
    ensure!(
        (1..=(PACKAGE_ENTRY_LIMIT as u64)).contains(&count),
        ArchiveSnafu
    );
    ensure!(
        bytes <= (ZIP_METADATA_LIMIT as u64) && start.checked_add(bytes) == Some(footer),
        ArchiveSnafu
    );
    let footer_bytes = length - footer;
    ensure!(
        bytes + footer_bytes <= (ZIP_METADATA_LIMIT as u64),
        ArchiveSnafu
    );
    Ok(Directory {
        start,
        bytes: bytes + footer_bytes,
        entries: usize::try_from(count).ok().context(ArchiveSnafu)?,
    })
}

fn zip64(file: &mut File, end: u64) -> Result<(u64, u64, u64, u64)> {
    let locator = read(file, end.checked_sub(20).context(ArchiveSnafu)?, 20)?;
    ensure!(
        &locator[..4] == b"PK\x06\x07"
            && u32::from_le_bytes(number(&locator, 4)?) == 0
            && u32::from_le_bytes(number(&locator, 16)?) == 1,
        ArchiveSnafu
    );
    let offset = u64::from_le_bytes(number(&locator, 8)?);
    let record = read(file, offset, 56)?;
    ensure!(&record[..4] == b"PK\x06\x06", ArchiveSnafu);
    let size = u64::from_le_bytes(number(&record, 4)?);
    ensure!(
        (44..=(ZIP_METADATA_LIMIT as u64)).contains(&size)
            && offset.checked_add(12).and_then(|v| v.checked_add(size)) == end.checked_sub(20),
        ArchiveSnafu
    );
    ensure!(
        u32::from_le_bytes(number(&record, 16)?) == 0
            && u32::from_le_bytes(number(&record, 20)?) == 0,
        ArchiveSnafu
    );
    let count = u64::from_le_bytes(number(&record, 32)?);
    ensure!(
        u64::from_le_bytes(number(&record, 24)?) == count,
        ArchiveSnafu
    );
    Ok((
        u64::from_le_bytes(number(&record, 48)?),
        u64::from_le_bytes(number(&record, 40)?),
        count,
        offset,
    ))
}
