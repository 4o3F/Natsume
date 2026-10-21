use std::{
    fs::{self, DirBuilder, File},
    io::{self, Read as _, Write as _},
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _},
    path::PathBuf,
};

use rustix::fs::{Mode, OFlags, open};
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, Snafu};

use super::{DOCUMENT_LIMIT, lowercase_hash};

#[derive(Clone)]
pub(crate) struct DocumentStore {
    root: PathBuf,
}

#[derive(Debug, Snafu)]
pub(crate) enum StorageError {
    #[snafu(display("print document filesystem operation failed"))]
    Io { source: io::Error },
    #[snafu(display("print document storage directory is unsafe"))]
    UnsafeDirectory,
    #[snafu(display("stored print document does not match its metadata"))]
    InvalidDocument,
}

impl DocumentStore {
    pub(crate) fn open(root: PathBuf) -> Result<Self, StorageError> {
        if !root.is_absolute() {
            return UnsafeDirectorySnafu.fail();
        }
        match DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(StorageError::Io { source }),
        }
        let metadata = fs::symlink_metadata(&root).context(IoSnafu)?;
        if !metadata.is_dir()
            || metadata.mode() & 0o022 != 0
            || (metadata.uid() != 0 && metadata.uid() != rustix::process::geteuid().as_raw())
        {
            return UnsafeDirectorySnafu.fail();
        }
        let parent = root.parent().ok_or(StorageError::UnsafeDirectory)?;
        File::open(parent)
            .context(IoSnafu)?
            .sync_all()
            .context(IoSnafu)?;
        Ok(Self { root })
    }

    fn path(&self, hash: &str) -> Result<PathBuf, StorageError> {
        if !lowercase_hash(hash) {
            return InvalidDocumentSnafu.fail();
        }
        Ok(self.root.join(format!("{hash}.pdf")))
    }

    pub(crate) fn verify(&self, hash: &str, bytes: i64) -> Result<(), StorageError> {
        let (_, content) = self.read(hash)?;
        if i64::try_from(content.len()).ok() != Some(bytes)
            || hex::encode(Sha256::digest(&content)) != hash
        {
            return InvalidDocumentSnafu.fail();
        }
        Ok(())
    }

    fn read(&self, hash: &str) -> Result<(File, Vec<u8>), StorageError> {
        let fd = open(
            self.path(hash)?,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)
        .context(IoSnafu)?;
        let file = File::from(fd);
        let metadata = file.metadata().context(IoSnafu)?;
        if !metadata.is_file() || metadata.len() > DOCUMENT_LIMIT as u64 {
            return InvalidDocumentSnafu.fail();
        }
        let mut bytes = Vec::new();
        (&file)
            .take(DOCUMENT_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .context(IoSnafu)?;
        if bytes.len() > DOCUMENT_LIMIT {
            return InvalidDocumentSnafu.fail();
        }
        Ok((file, bytes))
    }

    pub(crate) fn put(&self, hash: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let path = self.path(hash)?;
        if bytes.len() > DOCUMENT_LIMIT || hex::encode(Sha256::digest(bytes)) != hash {
            return InvalidDocumentSnafu.fail();
        }
        let mut temporary = tempfile::Builder::new()
            .prefix(".upload-")
            .tempfile_in(&self.root)
            .context(IoSnafu)?;
        temporary.write_all(bytes).context(IoSnafu)?;
        temporary.as_file().sync_all().context(IoSnafu)?;
        match temporary.persist_noclobber(path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                // Never replace an existing immutable document, including on replay.
                let (file, existing) = self.read(hash)?;
                if existing != bytes {
                    return InvalidDocumentSnafu.fail();
                }
                file.sync_all().context(IoSnafu)?;
            }
            Err(error) => return Err(error.error).context(IoSnafu),
        }
        // The database may reference the file only after its directory entry is durable.
        File::open(&self.root)
            .context(IoSnafu)?
            .sync_all()
            .context(IoSnafu)
    }
}
