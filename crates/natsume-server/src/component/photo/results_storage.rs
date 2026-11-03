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
    Corrupt,
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
    ensure!(directory.is_absolute(), CorruptSnafu);
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
        checked_directory(File::from(
            openat(&root, name, DIRECTORY, Mode::empty())
                .map_err(std::io::Error::from)
                .context(IoSnafu)?,
        ))?
        .sync_all()
        .context(IoSnafu)?;
    }
    root.sync_all().context(IoSnafu)?;
    File::open(directory.parent().ok_or_else(|| CorruptSnafu.build())?)
        .context(IoSnafu)?
        .sync_all()
        .context(IoSnafu)?;
    Ok(ResultsStore {
        root: Arc::new(root),
    })
}

impl ResultsStore {
    pub(super) fn create_upload(&self, id: uuid::Uuid) -> Result<UploadFile, ResultsStorageError> {
        let directory = checked_directory(File::from(
            openat(self.root.as_ref(), "uploads", DIRECTORY, Mode::empty())
                .map_err(std::io::Error::from)
                .context(IoSnafu)?,
        ))?;
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
        let directory = checked_directory(File::from(
            openat(self.root.as_ref(), "uploads", DIRECTORY, Mode::empty())
                .map_err(std::io::Error::from)
                .context(IoSnafu)?,
        ))?;
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

fn checked_directory(file: File) -> Result<File, ResultsStorageError> {
    let metadata = file.metadata().context(IoSnafu)?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && Mode::from_raw_mode(metadata.mode()).intersection(Mode::from_raw_mode(0o777))
                == Mode::RUSR | Mode::WUSR | Mode::XUSR,
        CorruptSnafu
    );
    Ok(file)
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
