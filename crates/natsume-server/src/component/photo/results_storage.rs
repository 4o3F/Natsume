//! Private result directories; no uploaded path becomes a filesystem path.
use rustix::fs::{Mode, OFlags, mkdirat, open, openat};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{
    fs::{DirBuilder, File},
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _},
    path::Path,
    sync::Arc,
};

const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

#[derive(Debug, Snafu)]
pub(crate) enum ResultsStorageError {
    #[snafu(display("Photo results storage directory is unsafe"))]
    UnsafeDirectory,
    #[snafu(display("Photo result file ownership or mode is unsafe"))]
    UnsafeFile,
    #[snafu(display("Photo result object length or hash disagrees"))]
    ObjectMismatch,
    #[snafu(display("Photo results storage operation failed"))]
    Io { source: std::io::Error },
}

#[derive(Clone)]
pub(super) struct ResultsStore {
    root: Arc<File>,
}

pub(super) struct UploadFile {
    pub file: File,
    pub directory: File,
}

pub(super) fn prepare(directory: &Path) -> Result<ResultsStore, ResultsStorageError> {
    ensure!(directory.is_absolute(), UnsafeDirectorySnafu);
    match DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(source) => return Err(source).context(IoSnafu),
    }
    let root = checked_directory(File::from(
        open(directory, DIRECTORY, Mode::empty())
            .map_err(std::io::Error::from)
            .context(IoSnafu)?,
    ))?;
    for name in ["uploads", "staging", "objects", "exports"] {
        match mkdirat(&root, name, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(source) => return Err(std::io::Error::from(source)).context(IoSnafu),
        }
        open_dir(&root, name)?.sync_all().context(IoSnafu)?;
    }
    root.sync_all().context(IoSnafu)?;
    File::open(
        directory
            .parent()
            .ok_or_else(|| UnsafeDirectorySnafu.build())?,
    )
    .context(IoSnafu)?
    .sync_all()
    .context(IoSnafu)?;
    Ok(ResultsStore {
        root: Arc::new(root),
    })
}

impl ResultsStore {
    pub(super) fn open_object(
        &self,
        photo: &super::delivery::PhotoRecord,
    ) -> Result<File, ResultsStorageError> {
        private_file(
            &self.directory("objects")?,
            &object_name(&photo.image_sha256, photo.image_format),
        )
    }

    pub(super) fn export_file(&self) -> Result<File, ResultsStorageError> {
        let directory = self.directory("exports")?;
        // Anonymous inode: closing the last descriptor cleans up even after a
        // disconnected download or process termination; no name is later unlinked.
        let file = File::from(
            openat(
                &directory,
                ".",
                OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)
            .context(IoSnafu)?,
        );
        Ok(file)
    }
    pub(super) fn worker_lease(&self) -> Result<Arc<File>, ResultsStorageError> {
        let file = open_dir(self.root.as_ref(), ".")?;
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(std::io::Error::from)
            .context(IoSnafu)?;
        Ok(Arc::new(file))
    }

    pub(super) fn open_upload(&self, id: uuid::Uuid) -> Result<File, ResultsStorageError> {
        let directory = self.directory("uploads")?;
        private_file(&directory, &format!("{id}.zip"))
    }

    fn directory(&self, name: &str) -> Result<File, ResultsStorageError> {
        open_dir(self.root.as_ref(), name)
    }

    pub(super) fn stage(&self) -> Result<Staging, ResultsStorageError> {
        let parent = self.directory("staging")?;
        let name = uuid::Uuid::now_v7().to_string();
        mkdirat(&parent, name.as_str(), Mode::RUSR | Mode::WUSR | Mode::XUSR)
            .map_err(std::io::Error::from)
            .context(IoSnafu)?;
        let directory = open_dir(&parent, &name)?;
        parent.sync_all().context(IoSnafu)?;
        Ok(Staging {
            parent,
            directory,
            name,
            files: Vec::new(),
            live: true,
        })
    }

    pub(super) fn publish(
        &self,
        stage: &Staging,
        image: &natsume_photo_protocol::results::TeamResult,
    ) -> Result<(), ResultsStorageError> {
        let objects = self.directory("objects")?;
        let name = object_name(&image.image_sha256, image.image_format);
        match rustix::fs::linkat(
            &stage.directory,
            name.as_str(),
            &objects,
            name.as_str(),
            rustix::fs::AtFlags::empty(),
        ) {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) => {
                let mut file = private_file(&objects, &name)?;
                let mut copied = 0;
                super::transfer::copy_verified(
                    &mut file,
                    &mut std::io::sink(),
                    super::transfer::Expectation::Image {
                        bytes: image.image_bytes,
                        sha256: &image.image_sha256,
                    },
                    &mut copied,
                    || true,
                )?;
            }
            Err(source) => return Err(std::io::Error::from(source)).context(IoSnafu),
        }
        Ok(())
    }

    pub(super) fn sync_objects(&self) -> Result<(), ResultsStorageError> {
        self.directory("objects")?.sync_all().context(IoSnafu)
    }
    pub(super) fn create_upload(&self, id: uuid::Uuid) -> Result<UploadFile, ResultsStorageError> {
        let directory = self.directory("uploads")?;
        let file = File::from(
            openat(
                &directory,
                format!("{id}.zip").as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)
            .context(IoSnafu)?,
        );
        Ok(UploadFile { file, directory })
    }

    pub(super) fn remove_upload(&self, id: uuid::Uuid) -> Result<(), ResultsStorageError> {
        let directory = self.directory("uploads")?;
        // A persisted task ID is not sufficient authority to unlink a substituted entry.
        let _file = private_file(&directory, &format!("{id}.zip"))?;
        rustix::fs::unlinkat(
            &directory,
            format!("{id}.zip").as_str(),
            rustix::fs::AtFlags::empty(),
        )
        .map_err(std::io::Error::from)
        .context(IoSnafu)?;
        directory.sync_all().context(IoSnafu)
    }
}

fn private_file(directory: &File, name: &str) -> Result<File, ResultsStorageError> {
    let file = File::from(
        openat(
            directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)
        .context(IoSnafu)?,
    );
    let metadata = file.metadata().context(IoSnafu)?;
    ensure!(
        metadata.is_file() && has_private_mode(&metadata, Mode::RUSR | Mode::WUSR),
        UnsafeFileSnafu
    );
    Ok(file)
}

fn object_name(
    hash: &natsume_photo_protocol::Sha256Digest,
    format: natsume_photo_protocol::results::PhotoFormat,
) -> String {
    format!("{hash}.{}", format.extension())
}

pub(super) struct Staging {
    parent: File,
    directory: File,
    name: String,
    files: Vec<String>,
    live: bool,
}

impl Staging {
    pub(super) fn create(
        &mut self,
        image: &natsume_photo_protocol::results::TeamResult,
    ) -> Result<File, ResultsStorageError> {
        let name = object_name(&image.image_sha256, image.image_format);
        let file = File::from(
            openat(
                &self.directory,
                name.as_str(),
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)
            .context(IoSnafu)?,
        );
        self.files.push(name);
        Ok(file)
    }

    fn clear(&mut self) -> Result<(), ResultsStorageError> {
        while let Some(name) = self.files.last() {
            rustix::fs::unlinkat(&self.directory, name.as_str(), rustix::fs::AtFlags::empty())
                .map_err(std::io::Error::from)
                .context(IoSnafu)?;
            self.files.pop();
        }
        rustix::fs::unlinkat(
            &self.parent,
            self.name.as_str(),
            rustix::fs::AtFlags::REMOVEDIR,
        )
        .map_err(std::io::Error::from)
        .context(IoSnafu)?;
        self.live = false;
        self.parent.sync_all().context(IoSnafu)
    }

    pub(super) fn cleanup(mut self) -> Result<(), ResultsStorageError> {
        self.clear()
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if self.live
            && let Err(error) = self.clear()
        {
            tracing::error!(%error, "Photo staging cleanup failed");
        }
    }
}

fn checked_directory(file: File) -> Result<File, ResultsStorageError> {
    let metadata = file.metadata().context(IoSnafu)?;
    ensure!(
        metadata.is_dir() && has_private_mode(&metadata, Mode::RUSR | Mode::WUSR | Mode::XUSR),
        UnsafeDirectorySnafu
    );
    Ok(file)
}

fn has_private_mode(metadata: &std::fs::Metadata, mode: Mode) -> bool {
    metadata.uid() == rustix::process::geteuid().as_raw()
        && Mode::from_raw_mode(metadata.mode()).intersection(Mode::from_raw_mode(0o777)) == mode
}

fn open_dir(parent: &File, name: &str) -> Result<File, ResultsStorageError> {
    checked_directory(File::from(
        openat(parent, name, DIRECTORY, Mode::empty())
            .map_err(std::io::Error::from)
            .context(IoSnafu)?,
    ))
}

impl From<super::transfer::TransferError> for ResultsStorageError {
    fn from(error: super::transfer::TransferError) -> Self {
        match error {
            super::transfer::TransferError::Read { source }
            | super::transfer::TransferError::Write { source } => Self::Io { source },
            _ => Self::ObjectMismatch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snafu::Snafu;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt as _, symlink},
    };

    #[derive(Debug, Snafu)]
    enum Failure {
        Io { source: std::io::Error },
        Storage { source: ResultsStorageError },
    }

    #[test]
    fn creates_private_directories_and_reopens_without_changing_files() -> Result<(), Failure> {
        let temp = tempfile::tempdir().context(IoSnafu)?;
        let root = temp.path().join("results");
        prepare(&root).context(StorageSnafu)?;
        fs::write(root.join("objects/retained"), b"unchanged").context(IoSnafu)?;
        prepare(&root).context(StorageSnafu)?;
        for name in ["", "uploads", "staging", "objects", "exports"] {
            let metadata = fs::metadata(root.join(name)).context(IoSnafu)?;
            assert!(metadata.is_dir());
            assert_eq!(metadata.mode() & 0o777, 0o700);
        }
        assert_eq!(
            fs::read(root.join("objects/retained")).context(IoSnafu)?,
            b"unchanged"
        );
        Ok(())
    }

    #[test]
    fn rejects_missing_owner_permissions_without_chmod() -> Result<(), Failure> {
        let temp = tempfile::tempdir().context(IoSnafu)?;
        let root = temp.path().join("results");
        prepare(&root).context(StorageSnafu)?;
        for name in ["", "uploads", "staging", "objects", "exports"] {
            for mode in [0o500, 0o600, 0o300] {
                let path = root.join(name);
                fs::set_permissions(&path, fs::Permissions::from_mode(mode)).context(IoSnafu)?;
                let rejected = prepare(&root).is_err();
                let actual = fs::metadata(&path).context(IoSnafu)?.mode() & 0o777;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).context(IoSnafu)?;
                assert!(rejected);
                assert_eq!(actual, mode);
            }
        }
        Ok(())
    }

    #[test]
    fn rejects_symlinks_and_shared_directories_without_changing_permissions() -> Result<(), Failure>
    {
        let temp = tempfile::tempdir().context(IoSnafu)?;
        let target = temp.path().join("target");
        fs::create_dir(&target).context(IoSnafu)?;
        let link = temp.path().join("link");
        symlink(&target, &link).context(IoSnafu)?;
        assert!(prepare(&link).is_err());
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).context(IoSnafu)?;
        assert!(prepare(&target).is_err());
        assert_eq!(
            fs::metadata(&target).context(IoSnafu)?.mode() & 0o777,
            0o755
        );
        let root = temp.path().join("results");
        prepare(&root).context(StorageSnafu)?;
        fs::remove_dir(root.join("objects")).context(IoSnafu)?;
        symlink(&target, root.join("objects")).context(IoSnafu)?;
        assert!(prepare(&root).is_err());
        Ok(())
    }
}
