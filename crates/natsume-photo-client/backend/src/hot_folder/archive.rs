//! Bounded, copy-only source capture. These files are not confirmed photos.
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    time::SystemTime,
};

use sha2::{Digest as _, Sha256};
use snafu::{IntoError as _, ResultExt as _, ensure};

use snafu::Snafu;

#[derive(Debug, Snafu)]
pub enum ArchiveError {
    #[snafu(display("source or archive file operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("source path or archive directory is not the expected local path"))]
    UnsafePath,
    #[snafu(display("source file changed while it was being read"))]
    Changed,
    #[snafu(display("source file exceeds the 50 MiB limit"))]
    Limit,
    #[snafu(display("existing archive content is corrupt and cannot be overwritten"))]
    InvalidArchive,
}
pub(super) type Result<T> = std::result::Result<T, ArchiveError>;

const MAX_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourceFileVersion {
    pub bytes: u64,
    pub modified: SystemTime,
    pub sha256: String,
}

fn open_source(path: &Path, hot_folder: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path).context(IoSnafu)?;
    ensure!(
        metadata.is_file() && !metadata.is_symlink(),
        UnsafePathSnafu
    );
    let resolved = fs::canonicalize(path).context(IoSnafu)?;
    ensure!(resolved.parent() == Some(hot_folder), UnsafePathSnafu);
    let mut options = OpenOptions::new();
    options.read(true);
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        // FILE_SHARE_READ: while this safe std handle is held, Windows rejects
        // concurrent writers and deletion/replacement. An existing writer makes
        // open fail with a sharing violation; the worker waits and retries.
        options.share_mode(1);
    }
    options.open(path).context(IoSnafu)
}

fn hash_into(file: &mut File, mut output: Option<&mut File>) -> Result<(u64, String)> {
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut hash = Sha256::new();
    let mut bytes = 0;
    loop {
        let count = file.read(&mut buffer).context(IoSnafu)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        ensure!(bytes <= MAX_BYTES, LimitSnafu);
        hash.update(&buffer[..count]);
        if let Some(destination) = &mut output {
            destination.write_all(&buffer[..count]).context(IoSnafu)?;
        }
    }
    Ok((bytes, hex::encode(hash.finalize())))
}

fn same_metadata(first: &Metadata, second: &Metadata) -> Result<bool> {
    Ok(first.len() == second.len()
        && first.modified().context(IoSnafu)? == second.modified().context(IoSnafu)?
        && first.created().ok() == second.created().ok())
}

pub(super) fn probe(path: &Path, hot_folder: &Path) -> Result<SourceFileVersion> {
    let mut file = open_source(path, hot_folder)?;
    let before = file.metadata().context(IoSnafu)?;
    ensure!(before.len() <= MAX_BYTES, LimitSnafu);
    let (bytes, sha256) = hash_into(&mut file, None)?;
    ensure!(
        bytes > 0 && same_metadata(&before, &file.metadata().context(IoSnafu)?)?,
        ChangedSnafu
    );
    Ok(SourceFileVersion {
        bytes,
        modified: before.modified().context(IoSnafu)?,
        sha256,
    })
}

pub(super) fn copy(
    source: &Path,
    hot_folder: &Path,
    archive: &Path,
    expected: &SourceFileVersion,
) -> Result<PathBuf> {
    // Reject a replaced archive directory, including links outside the workspace.
    ensure!(
        fs::canonicalize(archive).context(IoSnafu)? == archive,
        UnsafePathSnafu
    );
    let mut input = open_source(source, hot_folder)?;
    let before = input.metadata().context(IoSnafu)?;
    let mut temporary = tempfile::NamedTempFile::new_in(archive).context(IoSnafu)?;
    let (bytes, sha256) = hash_into(&mut input, Some(temporary.as_file_mut()))?;
    ensure!(
        bytes == expected.bytes
            && sha256 == expected.sha256
            && before.modified().context(IoSnafu)? == expected.modified
            && same_metadata(&before, &input.metadata().context(IoSnafu)?)?
            && same_metadata(&before, &fs::metadata(source).context(IoSnafu)?)?,
        ChangedSnafu
    );
    temporary.as_file().sync_all().context(IoSnafu)?;
    let destination = archive.join(format!("{sha256}.source"));
    match temporary.persist_noclobber(&destination) {
        Ok(file) => drop(file),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            // A previous observation can share bytes, never its team ownership.
            let existing = probe(&destination, archive)?;
            ensure!(
                existing.sha256 == sha256 && existing.bytes == bytes,
                InvalidArchiveSnafu
            );
        }
        Err(error) => {
            return Err(IoSnafu.into_error(error.error));
        }
    }
    Ok(destination)
}
