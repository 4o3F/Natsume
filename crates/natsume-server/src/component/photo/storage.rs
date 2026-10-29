//! Private, handle-relative immutable snapshot files.
use super::initialization::{CorruptSnafu, InitializationError, IoSnafu, WorkerSnafu};
use rustix::fs::{Mode, OFlags, mkdirat, open, openat};
use snafu::{ResultExt as _, ensure};
use std::{
    fs::{DirBuilder, File},
    io::{Read as _, Write as _},
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _},
    path::Path,
    sync::Arc,
};

const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

#[derive(Clone)]
pub(super) struct PhotoStore(Arc<File>);
pub(super) struct SnapshotFiles {
    root: Arc<File>,
    directory: Arc<File>,
}

impl PhotoStore {
    pub(super) fn open(path: &Path) -> Result<Self, InitializationError> {
        ensure!(path.is_absolute(), CorruptSnafu);
        match DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(source).context(IoSnafu),
        }
        let file = File::from(
            open(path, DIRECTORY, Mode::empty())
                .map_err(std::io::Error::from)
                .context(IoSnafu)?,
        );
        let metadata = file.metadata().context(IoSnafu)?;
        ensure!(
            metadata.is_dir()
                && !Mode::from_raw_mode(metadata.mode()).intersects(Mode::from_raw_mode(0o077))
                && metadata.uid() == rustix::process::geteuid().as_raw(),
            CorruptSnafu
        );
        File::open(path.parent().ok_or_else(|| CorruptSnafu.build())?)
            .context(IoSnafu)?
            .sync_all()
            .context(IoSnafu)?;
        Ok(Self(Arc::new(file)))
    }
    pub(super) async fn begin(&self, id: String) -> Result<SnapshotFiles, InitializationError> {
        let root = Arc::clone(&self.0);
        tokio::task::spawn_blocking(move || {
            ensure!(natsume_photo_protocol::id(&id), CorruptSnafu);
            mkdirat(
                root.as_ref(),
                id.as_str(),
                Mode::RUSR | Mode::WUSR | Mode::XUSR,
            )
            .map_err(std::io::Error::from)
            .context(IoSnafu)?;
            let directory = File::from(
                openat(root.as_ref(), id.as_str(), DIRECTORY, Mode::empty())
                    .map_err(std::io::Error::from)
                    .context(IoSnafu)?,
            );
            Ok(SnapshotFiles {
                root,
                directory: Arc::new(directory),
            })
        })
        .await
        .context(WorkerSnafu)?
    }
    pub(super) async fn read(
        &self,
        id: String,
        hash: String,
        size: u64,
    ) -> Result<Vec<u8>, InitializationError> {
        let root = Arc::clone(&self.0);
        tokio::task::spawn_blocking(move || {
            ensure!(
                natsume_photo_protocol::id(&id) && natsume_photo_protocol::hash(&hash),
                CorruptSnafu
            );
            let directory = openat(root.as_ref(), id.as_str(), DIRECTORY, Mode::empty())
                .map_err(std::io::Error::from)
                .context(IoSnafu)?;
            let file = File::from(
                openat(
                    &directory,
                    format!("{hash}.png").as_str(),
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(std::io::Error::from)
                .context(IoSnafu)?,
            );
            ensure!(
                file.metadata().context(IoSnafu)?.is_file()
                    && size <= natsume_photo_protocol::IMAGE_LIMIT as u64,
                CorruptSnafu
            );
            let mut bytes = Vec::new();
            file.take(natsume_photo_protocol::IMAGE_LIMIT as u64 + 1)
                .read_to_end(&mut bytes)
                .context(IoSnafu)?;
            ensure!(
                bytes.len() as u64 == size && hex::encode(super::enrollment::hash(&bytes)) == hash,
                CorruptSnafu
            );
            Ok(bytes)
        })
        .await
        .context(WorkerSnafu)?
    }
}

impl SnapshotFiles {
    pub(super) async fn put(
        &self,
        hash: String,
        bytes: Vec<u8>,
    ) -> Result<(), InitializationError> {
        let directory = Arc::clone(&self.directory);
        tokio::task::spawn_blocking(move || {
            ensure!(
                natsume_photo_protocol::hash(&hash)
                    && bytes.len() <= natsume_photo_protocol::IMAGE_LIMIT
                    && hex::encode(super::enrollment::hash(&bytes)) == hash,
                CorruptSnafu
            );
            let mut file = File::from(
                openat(
                    directory.as_ref(),
                    format!("{hash}.png").as_str(),
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                )
                .map_err(std::io::Error::from)
                .context(IoSnafu)?,
            );
            file.write_all(&bytes).context(IoSnafu)?;
            file.sync_all().context(IoSnafu)
        })
        .await
        .context(WorkerSnafu)?
    }
    pub(super) async fn finish(self) -> Result<(), InitializationError> {
        tokio::task::spawn_blocking(move || {
            self.directory.sync_all().context(IoSnafu)?;
            self.root.sync_all().context(IoSnafu)
        })
        .await
        .context(WorkerSnafu)?
    }
}
