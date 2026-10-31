use super::*;
use crate::workspace::SnapshotImage;
use natsume_photo_protocol::results::{
    PACKAGE_ENTRY_LIMIT, PACKAGE_LIMIT, TeamResult, ZIP_METADATA_LIMIT,
};
use sha2::{Digest as _, Sha256};
use std::io::{self, SeekFrom};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

struct Limited<W> {
    inner: W,
    position: u64,
}
impl<W: Write> Write for Limited<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > PACKAGE_LIMIT.saturating_sub(self.position) {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "photo ZIP exceeds byte limit",
            ));
        }
        let written = self.inner.write(bytes)?;
        self.position += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
impl<W: Seek> Seek for Limited<W> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let position = self.inner.seek(position)?;
        if position > PACKAGE_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "photo ZIP exceeds byte limit",
            ));
        }
        self.position = position;
        Ok(position)
    }
}

pub(super) fn write<W: Write + Seek>(
    file: W,
    manifest: &[u8],
    images: &mut [SnapshotImage],
) -> Result<()> {
    let mut zip = ZipWriter::new(Limited {
        inner: file,
        position: 0,
    });
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default());
    zip.start_file("manifest.json", options).context(ZipSnafu)?;
    zip.write_all(manifest).context(IoSnafu)?;
    for image in images {
        zip.start_file(image.entry.image_entry(), options)
            .context(ZipSnafu)?;
        image.file.rewind().context(IoSnafu)?;
        transfer(&mut image.file, Some(&mut zip), &image.entry)?;
    }
    zip.finish().context(ZipSnafu)?;
    Ok(())
}

fn transfer(
    reader: &mut impl Read,
    mut writer: Option<&mut dyn Write>,
    entry: &TeamResult,
) -> Result<()> {
    let mut buffer = vec![0; 64 * 1024];
    let mut hash = Sha256::new();
    let mut count = 0;
    loop {
        let read = reader.read(&mut buffer).context(IoSnafu)?;
        if read == 0 {
            break;
        }
        count += read as u64;
        ensure!(count <= entry.image_bytes, IntegritySnafu);
        hash.update(&buffer[..read]);
        if let Some(writer) = &mut writer {
            writer.write_all(&buffer[..read]).context(IoSnafu)?;
        }
    }
    ensure!(
        count == entry.image_bytes && hex::encode(hash.finalize()) == entry.image_sha256.as_str(),
        IntegritySnafu
    );
    Ok(())
}

pub(super) fn verify(file: &mut File, signed: &SignedResults) -> Result<()> {
    file.rewind().context(IoSnafu)?;
    let length = file.metadata().context(IoSnafu)?.len();
    ensure!(length <= PACKAGE_LIMIT, LimitSnafu);
    let json = signed.to_json().context(ContractSnafu)?;
    let mut entries = std::collections::BTreeMap::new();
    for entry in &signed.manifest.results {
        entries.insert(entry.image_entry(), entry);
    }
    let payload = json.len() as u64 + entries.values().map(|entry| entry.image_bytes).sum::<u64>();
    ensure!(
        length >= payload && length - payload <= ZIP_METADATA_LIMIT as u64,
        LimitSnafu
    );
    let mut zip = ZipArchive::new(file).context(ZipSnafu)?;
    ensure!(
        zip.len() == entries.len() + 1 && zip.len() <= PACKAGE_ENTRY_LIMIT,
        IntegritySnafu
    );
    for index in 0..zip.len() {
        let mut item = zip.by_index(index).context(ZipSnafu)?;
        let name = item.name().context(ZipSnafu)?.into_owned();
        ensure!(
            item.compression() == CompressionMethod::Stored,
            IntegritySnafu
        );
        if index == 0 {
            ensure!(
                name == "manifest.json" && item.size() == json.len() as u64,
                IntegritySnafu
            );
            let mut bytes = Vec::with_capacity(json.len());
            item.take(json.len() as u64 + 1)
                .read_to_end(&mut bytes)
                .context(IoSnafu)?;
            ensure!(bytes == json, IntegritySnafu);
        } else {
            let entry = entries
                .remove(&name)
                .ok_or_else(|| IntegritySnafu.build())?;
            ensure!(item.size() == entry.image_bytes, IntegritySnafu);
            transfer(&mut item, None, entry)?;
        }
    }
    ensure!(entries.is_empty(), IntegritySnafu);
    signed
        .verify(
            &signed.manifest.context,
            &signed.manifest.signing_public_key,
        )
        .context(ContractSnafu)
}

pub(super) fn digest(file: &mut File) -> Result<(u64, Sha256Digest)> {
    file.rewind().context(IoSnafu)?;
    let mut count = 0;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).context(IoSnafu)?;
        if read == 0 {
            break;
        }
        count += read as u64;
        ensure!(count <= PACKAGE_LIMIT, LimitSnafu);
        hash.update(&buffer[..read]);
    }
    let digest = hex::encode(hash.finalize())
        .parse()
        .map_err(|_| IntegritySnafu.build())?;
    Ok((count, digest))
}
