//! ZIP content validation; never extracts a caller-supplied path.
use super::{
    AllocationSnafu, ArchiveSnafu, ImageSnafu, IntegritySnafu, InterruptedSnafu, IoSnafu,
    JsonSnafu, Result, StorageSnafu, ZipSnafu, directory,
};
use crate::component::photo::results_storage::Staging;
use image::{ImageDecoder as _, ImageFormat, ImageReader, Limits};
use natsume_photo_protocol::{
    MANIFEST_LIMIT,
    results::{PACKAGE_LIMIT, RESULT_PIXEL_LIMIT, SignedResults, TeamResult, ZIP_METADATA_LIMIT},
};
use snafu::{OptionExt as _, ResultExt as _, ensure};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs::File,
    io::{self, BufReader, Read as _, Seek as _},
    rc::Rc,
};
use tokio_util::sync::CancellationToken;
use zip::{CompressionMethod, ZipArchive, ZipReadOptions};

pub(super) struct Package {
    zip: ZipArchive<MetadataReader>,
    names: BTreeMap<String, usize>,
    pub signed: std::sync::Arc<SignedResults>,
}

// zip may fall back to an earlier EOCD after a malformed directory. Bound all
// constructor reads as well, so that fallback cannot bypass our footer preflight.
struct MetadataReader {
    file: File,
    parsing: Rc<Cell<bool>>,
    remaining: u64,
}
impl io::Read for MetadataReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !self.parsing.get() {
            return self.file.read(buffer);
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Photo ZIP metadata read budget exceeded",
            ));
        }
        let capacity = buffer
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let count = self.file.read(&mut buffer[..capacity])?;
        self.remaining -= count as u64;
        Ok(count)
    }
}
impl io::Seek for MetadataReader {
    fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}

impl Package {
    pub(super) fn open(mut file: File, expected_bytes: u64) -> Result<Self> {
        let directory = directory::preflight(&mut file, expected_bytes)?;
        file.rewind().context(IoSnafu)?;
        let parsing = Rc::new(Cell::new(true));
        let reader = MetadataReader {
            file,
            parsing: parsing.clone(),
            remaining: (ZIP_METADATA_LIMIT as u64) * 2
                + super::super::limits::ZIP_EOCD_SCAN_BYTES * 2,
        };
        let mut zip = ZipArchive::new(reader).context(ZipSnafu)?;
        parsing.set(false);
        ensure!(
            zip.offset() == 0
                && zip.central_directory_start() == directory.start
                && zip.len() == directory.entries
                && !zip.has_overlapping_entries().context(ZipSnafu)?,
            ArchiveSnafu
        );
        let mut names = BTreeMap::new();
        let mut metadata_bytes = directory.bytes;
        let mut unpacked = 0_u64;
        for index in 0..zip.len() {
            let entry = zip
                .by_index_with_options(
                    index,
                    ZipReadOptions::new().enforce_header_consistency(true),
                )
                .context(ZipSnafu)?;
            ensure!(
                entry.is_file()
                    && !entry.encrypted()
                    && matches!(
                        entry.compression(),
                        CompressionMethod::Stored | CompressionMethod::Deflated
                    ),
                ArchiveSnafu
            );
            ensure!(
                entry
                    .unix_mode()
                    .is_none_or(|mode| matches!(mode & 0o170_000, 0 | 0o100_000)),
                ArchiveSnafu
            );
            let data = entry.data_start().context(ArchiveSnafu)?;
            let header = entry.header_start();
            ensure!(
                data >= header
                    && data
                        .checked_add(entry.compressed_size())
                        .is_some_and(|end| end <= directory.start),
                ArchiveSnafu
            );
            metadata_bytes = metadata_bytes
                .checked_add(data - header)
                .context(ArchiveSnafu)?;
            unpacked = unpacked.checked_add(entry.size()).context(ArchiveSnafu)?;
            ensure!(
                metadata_bytes <= (ZIP_METADATA_LIMIT as u64) && unpacked <= PACKAGE_LIMIT,
                ArchiveSnafu
            );
            let name = entry.name().context(ZipSnafu)?.into_owned();
            ensure!(names.insert(name, index).is_none(), ArchiveSnafu);
        }
        let index = *names.get("manifest.json").context(ArchiveSnafu)?;
        let mut entry = zip
            .by_index_with_options(
                index,
                ZipReadOptions::new().enforce_header_consistency(true),
            )
            .context(ZipSnafu)?;
        ensure!(entry.size() <= (MANIFEST_LIMIT as u64), ArchiveSnafu);
        let expected = entry.size();
        let mut raw = Vec::new();
        entry
            .by_ref()
            .take((MANIFEST_LIMIT as u64) + 1)
            .read_to_end(&mut raw)
            .context(IoSnafu)?;
        ensure!(raw.len() as u64 == expected, ArchiveSnafu);
        let signed = std::sync::Arc::new(serde_json::from_slice(&raw).context(JsonSnafu)?);
        drop(entry);
        Ok(Self { zip, names, signed })
    }

    pub(super) fn images(
        &mut self,
        stage: &mut Staging,
        stop: &CancellationToken,
    ) -> Result<Vec<TeamResult>> {
        let images: BTreeMap<_, _> = self
            .signed
            .manifest
            .results
            .iter()
            .map(|entry| (entry.image_entry(), entry.clone()))
            .collect();
        ensure!(
            self.names.len() == images.len() + 1
                && self
                    .names
                    .keys()
                    .all(|name| name == "manifest.json" || images.contains_key(name)),
            ArchiveSnafu
        );
        for (name, expected) in &images {
            ensure!(!stop.is_cancelled(), InterruptedSnafu);
            let mut source = self
                .zip
                .by_index_with_options(
                    *self.names.get(name).context(ArchiveSnafu)?,
                    ZipReadOptions::new().enforce_header_consistency(true),
                )
                .context(ZipSnafu)?;
            ensure!(source.size() == expected.image_bytes, IntegritySnafu);
            let mut target = stage.create(expected).context(StorageSnafu)?;
            let mut total = 0;
            super::super::transfer::copy_verified(
                &mut source,
                &mut target,
                super::super::transfer::Expectation::Image {
                    bytes: expected.image_bytes,
                    sha256: &expected.image_sha256,
                },
                &mut total,
                || !stop.is_cancelled(),
            )?;
            target.sync_all().context(IoSnafu)?;
            decode(&mut target, expected)?;
        }
        Ok(images.into_values().collect())
    }
}

fn decode(file: &mut File, expected: &TeamResult) -> Result<()> {
    file.rewind().context(IoSnafu)?;
    let mut reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .context(IoSnafu)?;
    let format = match expected.image_format {
        natsume_photo_protocol::results::PhotoFormat::Jpeg => ImageFormat::Jpeg,
        natsume_photo_protocol::results::PhotoFormat::Png => ImageFormat::Png,
    };
    ensure!(reader.format() == Some(format), IntegritySnafu);
    let mut limits = Limits::default();
    limits.max_alloc = Some(crate::component::photo::limits::IMAGE_DECODE_BYTES);
    reader.limits(limits);
    let decoder = reader.into_decoder().context(ImageSnafu)?;
    ensure!(
        decoder.dimensions() == (expected.width, expected.height)
            && u64::from(expected.width) * u64::from(expected.height) <= RESULT_PIXEL_LIMIT
            && decoder.total_bytes() <= crate::component::photo::limits::IMAGE_DECODE_BYTES,
        IntegritySnafu
    );
    let count = usize::try_from(decoder.total_bytes()).map_err(|_| IntegritySnafu.build())?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(count).context(AllocationSnafu)?;
    pixels.resize(count, 0);
    decoder.read_image(&mut pixels).context(ImageSnafu)
}
