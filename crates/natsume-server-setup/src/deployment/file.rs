//! Atomic deployment file operations for first-install setup.

use std::{
    fs::File,
    io::{self, Read as _, Write as _},
    os::unix::fs::MetadataExt as _,
    path::{Component, Path, PathBuf},
};

use rustix::fs::{self as unix_fs, AtFlags, FileType, Gid, Mode, OFlags, RenameFlags, Uid};
use sha2::{Digest as _, Sha256};
use snafu::{OptionExt as _, ResultExt as _, Snafu, ensure};
use zeroize::Zeroizing;

/// Numeric Unix ownership applied to newly created deployment entries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ownership {
    /// User ID of the entry owner.
    pub uid: u32,
    /// Group ID of the entry owner.
    pub gid: u32,
}

/// Root ownership required for administrator signing material.
pub const ROOT_OWNER: Ownership = Ownership { uid: 0, gid: 0 };

/// Destination and final metadata for a newly installed file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileSpec {
    /// Destination path, which must not already exist.
    pub path: PathBuf,
    /// Ownership applied before publishing the file.
    pub owner: Ownership,
    /// Final Unix permission bits.
    pub mode: u32,
}

/// Identity and content fingerprint used to validate a file before rollback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatedFile {
    /// Installed file path.
    pub path: PathBuf,
    /// Filesystem device identifier at creation.
    pub device: u64,
    /// Inode number at creation.
    pub inode: u64,
    /// SHA-256 digest of the installed contents.
    pub sha256: [u8; 32],
}

/// A deployment filesystem operation could not be completed safely.
#[derive(Debug, Snafu)]
pub enum FileError {
    /// The destination is already occupied, including by a dangling symlink.
    #[snafu(display("destination already exists: {}", path.display()))]
    AlreadyExists {
        /// Occupied destination that was left unchanged.
        path: PathBuf,
    },
    /// A directory path resolves to another entry type.
    #[snafu(display("path is not a directory: {}", path.display()))]
    NotADirectory {
        /// Path containing a non-directory entry.
        path: PathBuf,
    },
    /// A directory path contains a symbolic link.
    #[snafu(display("symbolic link rejected: {}", path.display()))]
    SymlinkRejected {
        /// Path containing a rejected symbolic link.
        path: PathBuf,
    },
    /// The rollback target no longer has its original identity or contents.
    #[snafu(display("file changed since creation: {}", path.display()))]
    ChangedSinceCreation {
        /// Path whose identity or contents differ from the creation record.
        path: PathBuf,
    },
    /// A filesystem path has an invalid shape or contains forbidden parent traversal.
    #[snafu(display("invalid filesystem path: {}", path.display()))]
    InvalidPath {
        /// Rejected directory or file path.
        path: PathBuf,
    },
    /// An absolute administrator PKI path could not be rooted.
    #[snafu(display("cannot resolve administrator PKI path"))]
    PathPrefix {
        /// Original path normalization cause.
        source: std::path::StripPrefixError,
    },
    /// A protected input has unsafe ownership, permissions, or entry type.
    #[snafu(display("unsafe input ownership, permissions, or type: {}", path.display()))]
    Metadata {
        /// Input with rejected metadata.
        path: PathBuf,
    },
    /// An input exceeds its bounded read allowance.
    #[snafu(display("input too large: {}", path.display()))]
    TooLarge {
        /// Oversized input path.
        path: PathBuf,
    },
    /// The underlying filesystem operation failed.
    #[snafu(display("filesystem operation failed for {}: {source}", path.display()))]
    Io {
        /// Path affected by the filesystem failure.
        path: PathBuf,
        /// Original filesystem cause, excluding supplied file contents.
        source: io::Error,
    },
}

/// Creates missing directories and returns their paths from outermost to innermost.
///
/// Existing directories retain their ownership and permissions.
///
/// # Errors
/// Rejects symbolic links, non-directory components, and filesystem failures.
pub fn ensure_directory(
    path: &Path,
    owner: Ownership,
    mode: u32,
) -> Result<Vec<PathBuf>, FileError> {
    walk_directory(path, Some((owner, mode))).map(|(_, created)| created)
}

fn walk_directory(
    path: &Path,
    create: Option<(Ownership, u32)>,
) -> Result<(File, Vec<PathBuf>), FileError> {
    let mut current = PathBuf::from(if path.is_absolute() { "/" } else { "." });
    let mut directory = File::from(
        unix_fs::open(
            &current,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)
        .context(IoSnafu { path: &current })?,
    );
    let mut created = Vec::new();
    for component in path.components() {
        if matches!(component, Component::RootDir | Component::CurDir) {
            continue;
        }
        let name = component.as_os_str();
        if current == Path::new(".") {
            current = PathBuf::from(name);
        } else {
            current.push(name);
        }
        let mut is_new = false;
        match unix_fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => {
                ensure!(
                    FileType::from_raw_mode(stat.st_mode) != FileType::Symlink,
                    SymlinkRejectedSnafu { path: &current }
                );
                ensure!(
                    FileType::from_raw_mode(stat.st_mode) == FileType::Directory,
                    NotADirectorySnafu { path: &current }
                );
            }
            Err(rustix::io::Errno::NOENT) if create.is_some() => {
                match unix_fs::mkdirat(&directory, name, Mode::RWXU) {
                    Ok(()) => is_new = true,
                    Err(rustix::io::Errno::EXIST) => {}
                    Err(source) => {
                        return Err(io::Error::from(source)).context(IoSnafu { path: &current });
                    }
                }
            }
            Err(source) => {
                return Err(io::Error::from(source)).context(IoSnafu { path: &current });
            }
        }
        let child = unix_fs::openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(io::Error::from)
        .context(IoSnafu { path: &current })?;
        if let Some((owner, mode)) = create.filter(|_| is_new) {
            set_metadata(&child, &current, owner, mode)?;
            created.push(current.clone());
        }
        directory = child;
    }
    Ok((directory, created))
}

fn set_metadata(file: &File, path: &Path, owner: Ownership, mode: u32) -> Result<(), FileError> {
    unix_fs::fchown(
        file,
        Some(Uid::from_raw(owner.uid)),
        Some(Gid::from_raw(owner.gid)),
    )
    .map_err(io::Error::from)
    .context(IoSnafu { path })?;
    unix_fs::fchmod(file, Mode::from_bits_truncate(mode))
        .map_err(io::Error::from)
        .context(IoSnafu { path })
}

fn parent_directory(path: &Path) -> Result<(File, &Path), FileError> {
    let name = path.file_name().context(InvalidPathSnafu { path })?;
    let parent = path.parent().context(InvalidPathSnafu { path })?;
    let (directory, _) = walk_directory(parent, None)?;
    Ok((directory, Path::new(name)))
}

/// Installs contents as a new file without ever including the supplied bytes in diagnostics.
///
/// # Errors
/// Rejects existing destinations, unsafe or missing parents, and filesystem failures.
pub fn create_new_file(spec: &FileSpec, contents: &[u8]) -> Result<CreatedFile, FileError> {
    let path = &spec.path;
    let (directory, name) = parent_directory(path)?;
    match unix_fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => return AlreadyExistsSnafu { path }.fail(),
        Err(rustix::io::Errno::NOENT) => {}
        Err(source) => return Err(io::Error::from(source)).context(IoSnafu { path }),
    }
    let parent = path.parent().context(InvalidPathSnafu { path })?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let temporary = tempfile::Builder::new()
        .prefix(".natsume-setup-")
        .disable_cleanup(true)
        .make_in(parent, |temporary_path| {
            let name = temporary_path
                .file_name()
                .ok_or(io::ErrorKind::InvalidInput)?;
            let fd = unix_fs::openat(
                &directory,
                name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?;
            Ok(TemporaryFile {
                file: File::from(fd),
                directory: &directory,
                name: PathBuf::from(name),
            })
        })
        .context(IoSnafu { path })?;
    let mut temporary = temporary.into_parts().0;
    temporary
        .file
        .write_all(contents)
        .context(IoSnafu { path })?;
    set_metadata(&temporary.file, path, spec.owner, spec.mode)?;
    temporary.file.sync_all().context(IoSnafu { path })?;
    let metadata = temporary.file.metadata().context(IoSnafu { path })?;
    publish_file(&directory, &temporary.name, name, path)?;
    drop(temporary);
    directory.sync_all().context(IoSnafu { path })?;
    Ok(CreatedFile {
        path: path.clone(),
        device: metadata.dev(),
        inode: metadata.ino(),
        sha256: Sha256::digest(contents).into(),
    })
}

struct TemporaryFile<'directory> {
    file: File,
    directory: &'directory File,
    name: PathBuf,
}

impl Drop for TemporaryFile<'_> {
    fn drop(&mut self) {
        match unix_fs::unlinkat(self.directory, &self.name, AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => {}
            Err(source) => tracing::warn!(
                name = ?self.name,
                %source,
                "failed to remove deployment temporary file"
            ),
        }
    }
}

fn publish_file(
    directory: &File,
    temporary: &Path,
    name: &Path,
    path: &Path,
) -> Result<(), FileError> {
    match unix_fs::renameat_with(
        directory,
        temporary,
        directory,
        name,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::EXIST) => AlreadyExistsSnafu { path }.fail(),
        Err(rustix::io::Errno::NOSYS | rustix::io::Errno::INVAL | rustix::io::Errno::OPNOTSUPP) => {
            link_file(directory, temporary, name, path)
        }
        Err(source) => Err(io::Error::from(source)).context(IoSnafu { path }),
    }
}

fn link_file(
    directory: &File,
    temporary: &Path,
    name: &Path,
    path: &Path,
) -> Result<(), FileError> {
    match unix_fs::linkat(directory, temporary, directory, name, AtFlags::empty()) {
        Ok(()) => {}
        Err(rustix::io::Errno::EXIST) => return AlreadyExistsSnafu { path }.fail(),
        Err(source) => return Err(io::Error::from(source)).context(IoSnafu { path }),
    }
    unix_fs::unlinkat(directory, temporary, AtFlags::empty())
        .map_err(io::Error::from)
        .context(IoSnafu { path })
}

/// Removes an installed file only while its identity and contents match creation.
///
/// # Errors
/// Returns `ChangedSinceCreation` for replaced or modified files, otherwise filesystem errors.
pub fn remove_created_file(created: &CreatedFile) -> Result<(), FileError> {
    let path = &created.path;
    let (directory, name) = parent_directory(path)?;
    let before = unix_fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(io::Error::from)
        .context(IoSnafu { path })?;
    ensure!(
        FileType::from_raw_mode(before.st_mode) == FileType::RegularFile
            && before.st_dev == created.device
            && before.st_ino == created.inode,
        ChangedSinceCreationSnafu { path }
    );
    let mut file = File::from(
        unix_fs::openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)
        .context(IoSnafu { path })?,
    );
    let metadata = file.metadata().context(IoSnafu { path })?;
    ensure!(
        metadata.is_file() && metadata.dev() == created.device && metadata.ino() == created.inode,
        ChangedSinceCreationSnafu { path }
    );
    let mut hash = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let read = file.read(&mut buffer).context(IoSnafu { path })?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    let digest: [u8; 32] = hash.finalize().into();
    let current = unix_fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(io::Error::from)
        .context(IoSnafu { path })?;
    ensure!(
        digest == created.sha256
            && FileType::from_raw_mode(current.st_mode) == FileType::RegularFile
            && current.st_dev == created.device
            && current.st_ino == created.inode,
        ChangedSinceCreationSnafu { path }
    );
    unix_fs::unlinkat(&directory, name, AtFlags::empty())
        .map_err(io::Error::from)
        .context(IoSnafu { path })?;
    directory.sync_all().context(IoSnafu { path })
}

/// Result of attempting to remove a directory created by this deployment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryRemoval {
    /// The empty directory was removed.
    Removed,
    /// Other entries remain in the directory, so it was preserved.
    KeptNotEmpty,
}

/// Removes an empty created directory, preserving nonempty directories as an expected outcome.
///
/// # Errors
/// Rejects symbolic links, non-directory entries, and filesystem failures.
pub fn remove_created_directory(path: &Path) -> Result<DirectoryRemoval, FileError> {
    let (directory, name) = parent_directory(path)?;
    let metadata = unix_fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(io::Error::from)
        .context(IoSnafu { path })?;
    ensure!(
        FileType::from_raw_mode(metadata.st_mode) != FileType::Symlink,
        SymlinkRejectedSnafu { path }
    );
    ensure!(
        FileType::from_raw_mode(metadata.st_mode) == FileType::Directory,
        NotADirectorySnafu { path }
    );
    match unix_fs::unlinkat(&directory, name, AtFlags::REMOVEDIR) {
        Ok(()) => {}
        Err(rustix::io::Errno::NOTEMPTY) => return Ok(DirectoryRemoval::KeptNotEmpty),
        Err(source) => return Err(io::Error::from(source)).context(IoSnafu { path }),
    }
    directory.sync_all().context(IoSnafu { path })?;
    Ok(DirectoryRemoval::Removed)
}

/// Resolves an absolute administrator PKI path beneath the selected target root.
///
/// # Errors
/// Rejects relative paths and parent traversal, retaining any prefix-removal failure.
pub(crate) fn rooted_pki(target_root: &Path, pki_dir: &Path) -> Result<PathBuf, FileError> {
    ensure!(
        pki_dir.is_absolute()
            && pki_dir
                .components()
                .all(|part| !matches!(part, Component::ParentDir)),
        InvalidPathSnafu { path: pki_dir }
    );
    Ok(target_root.join(pki_dir.strip_prefix("/").context(PathPrefixSnafu)?))
}

fn open_directory(path: &Path) -> Result<File, FileError> {
    ensure!(
        !path
            .components()
            .any(|part| matches!(part, Component::ParentDir)),
        InvalidPathSnafu { path }
    );
    walk_directory(path, None).map(|(directory, _)| directory)
}

/// Verifies ownership and exact permissions through a symlink-resistant directory walk.
///
/// # Errors
/// Rejects parent traversal, symbolic links, unsafe metadata, and filesystem failures.
pub(crate) fn check_directory(path: &Path, owner: Ownership, mode: u32) -> Result<(), FileError> {
    let metadata = open_directory(path)?.metadata().context(IoSnafu { path })?;
    ensure!(
        metadata.uid() == owner.uid
            && metadata.gid() == owner.gid
            && metadata.mode() & 0o7777 == mode,
        MetadataSnafu { path }
    );
    Ok(())
}

/// Reads a protected regular file within the byte limit and records its identity and digest.
///
/// # Errors
/// Rejects symbolic links, unsafe metadata, oversized inputs, and filesystem failures.
pub(crate) fn read_file(
    path: &Path,
    owner: Ownership,
    mode: u32,
    max_bytes: u64,
) -> Result<(Zeroizing<Vec<u8>>, CreatedFile), FileError> {
    let parent = path.parent().context(InvalidPathSnafu { path })?;
    let name = path.file_name().context(InvalidPathSnafu { path })?;
    let directory = open_directory(parent)?;
    let file = File::from(
        unix_fs::openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)
        .context(IoSnafu { path })?,
    );
    let metadata = file.metadata().context(IoSnafu { path })?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == owner.uid
            && metadata.gid() == owner.gid
            && metadata.mode() & 0o7777 == mode,
        MetadataSnafu { path }
    );
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu { path })?;
    ensure!(bytes.len() as u64 <= max_bytes, TooLargeSnafu { path });
    let created = CreatedFile {
        path: path.into(),
        device: metadata.dev(),
        inode: metadata.ino(),
        sha256: Sha256::digest(&bytes).into(),
    };
    Ok((bytes, created))
}

/// Reverses only recorded creations after a failed pending or handoff publication.
pub(crate) fn cleanup(files: &[CreatedFile], directories: &[PathBuf]) {
    for file in files.iter().rev() {
        if let Err(error) = remove_created_file(file) {
            tracing::error!(error = %snafu::Report::from_error(error), "cannot remove incomplete setup file");
        }
    }
    for directory in directories.iter().rev() {
        if let Err(error) = remove_created_directory(directory) {
            tracing::error!(error = %snafu::Report::from_error(error), "cannot remove incomplete setup directory");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{error::Error as _, fs, os::unix::fs::PermissionsExt as _};

    use tempfile::TempDir;

    use super::*;

    #[derive(Debug, Snafu)]
    enum TestError {
        #[snafu(display("fixture I/O failed: {source}"))]
        TestIo { source: io::Error },
        #[snafu(display("deployment operation failed: {source}"))]
        TestFile { source: FileError },
    }

    type TestResult<T = ()> = Result<T, TestError>;

    fn ownership() -> Ownership {
        Ownership {
            uid: rustix::process::geteuid().as_raw(),
            gid: rustix::process::getegid().as_raw(),
        }
    }

    fn fixture() -> TestResult<(TempDir, FileSpec)> {
        let directory = tempfile::tempdir().context(TestIoSnafu)?;
        let spec = FileSpec {
            path: directory.path().join("installed"),
            owner: ownership(),
            mode: 0o640,
        };
        Ok((directory, spec))
    }

    #[test]
    fn protected_reads_enforce_metadata_and_size_and_record_identity() -> TestResult {
        let (directory, spec) = fixture()?;
        let created = create_new_file(&spec, b"contents").context(TestFileSnafu)?;
        let (bytes, read) =
            read_file(&spec.path, spec.owner, spec.mode, 8).context(TestFileSnafu)?;
        assert_eq!(bytes.as_slice(), b"contents");
        assert_eq!(read, created);
        assert!(matches!(
            read_file(&spec.path, spec.owner, spec.mode, 7),
            Err(FileError::TooLarge { .. })
        ));
        assert!(matches!(
            read_file(&spec.path, spec.owner, 0o600, 8),
            Err(FileError::Metadata { .. })
        ));
        let other_owner = Ownership {
            uid: spec.owner.uid ^ 1,
            ..spec.owner
        };
        assert!(matches!(
            read_file(&spec.path, other_owner, spec.mode, 8),
            Err(FileError::Metadata { .. })
        ));
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .context(TestIoSnafu)?;
        check_directory(directory.path(), spec.owner, 0o700).context(TestFileSnafu)?;
        assert!(matches!(
            check_directory(directory.path(), spec.owner, 0o755),
            Err(FileError::Metadata { .. })
        ));
        Ok(())
    }

    #[test]
    fn protected_reads_reject_symlinks_and_parent_traversal() -> TestResult {
        let (directory, spec) = fixture()?;
        create_new_file(&spec, b"contents").context(TestFileSnafu)?;
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(directory.path(), &link).context(TestIoSnafu)?;
        assert!(matches!(
            check_directory(&link, spec.owner, 0o700),
            Err(FileError::SymlinkRejected { .. })
        ));
        assert!(matches!(
            read_file(&link.join("installed"), spec.owner, spec.mode, 8),
            Err(FileError::SymlinkRejected { .. })
        ));
        let file_link = directory.path().join("file-link");
        std::os::unix::fs::symlink(&spec.path, &file_link).context(TestIoSnafu)?;
        let Err(error) = read_file(&file_link, spec.owner, spec.mode, 8) else {
            panic!("protected read accepted a file symlink");
        };
        assert!(
            error.source().is_some(),
            "symlink open failure must retain its source"
        );
        for path in [Path::new("relative"), Path::new("/pki/../elsewhere")] {
            assert!(matches!(
                rooted_pki(directory.path(), path),
                Err(FileError::InvalidPath { .. })
            ));
        }
        assert_eq!(
            rooted_pki(directory.path(), Path::new("/pki")).context(TestFileSnafu)?,
            directory.path().join("pki")
        );
        assert!(matches!(
            check_directory(&directory.path().join(".."), spec.owner, 0o700),
            Err(FileError::InvalidPath { .. })
        ));
        assert!(matches!(
            read_file(
                &directory.path().join("../installed"),
                spec.owner,
                spec.mode,
                8
            ),
            Err(FileError::InvalidPath { .. })
        ));
        Ok(())
    }

    #[test]
    fn creates_complete_file_with_final_metadata() -> TestResult {
        let (directory, spec) = fixture()?;
        let contents = vec![b'a'; 128 * 1024];
        let created = create_new_file(&spec, &contents).context(TestFileSnafu)?;
        let metadata = fs::metadata(&spec.path).context(TestIoSnafu)?;
        assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, contents);
        assert_eq!(metadata.mode() & 0o7777, spec.mode);
        assert_eq!(metadata.uid(), spec.owner.uid);
        assert_eq!(metadata.gid(), spec.owner.gid);
        assert_eq!(created.path, spec.path);
        assert_eq!(created.device, metadata.dev());
        assert_eq!(created.inode, metadata.ino());
        assert_eq!(created.sha256, <[u8; 32]>::from(Sha256::digest(&contents)));
        assert_eq!(
            fs::read_dir(directory.path()).context(TestIoSnafu)?.count(),
            1
        );
        Ok(())
    }

    #[test]
    fn rejects_existing_file_without_changing_it() -> TestResult {
        let (directory, spec) = fixture()?;
        fs::write(&spec.path, b"original").context(TestIoSnafu)?;
        let before = fs::metadata(&spec.path).context(TestIoSnafu)?;
        assert!(matches!(
            create_new_file(&spec, b"replacement"),
            Err(FileError::AlreadyExists { .. })
        ));
        assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, b"original");
        let after = fs::metadata(&spec.path).context(TestIoSnafu)?;
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before.mode(), after.mode());
        assert_eq!(
            fs::read_dir(directory.path()).context(TestIoSnafu)?.count(),
            1
        );
        Ok(())
    }

    #[test]
    fn rejects_existing_and_dangling_target_symlinks() -> TestResult {
        let (directory, spec) = fixture()?;
        let referent = directory.path().join("referent");
        for exists in [false, true] {
            if exists {
                fs::write(&referent, b"original").context(TestIoSnafu)?;
            }
            std::os::unix::fs::symlink(&referent, &spec.path).context(TestIoSnafu)?;
            assert!(matches!(
                create_new_file(&spec, b"replacement"),
                Err(FileError::AlreadyExists { .. })
            ));
            assert!(
                fs::symlink_metadata(&spec.path)
                    .context(TestIoSnafu)?
                    .is_symlink()
            );
            fs::remove_file(&spec.path).context(TestIoSnafu)?;
        }
        assert_eq!(fs::read(referent).context(TestIoSnafu)?, b"original");
        Ok(())
    }

    #[test]
    fn rejects_symlink_parent_and_ancestor() -> TestResult {
        let (directory, mut spec) = fixture()?;
        let real = directory.path().join("real");
        fs::create_dir_all(real.join("child")).context(TestIoSnafu)?;
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&real, &link).context(TestIoSnafu)?;
        for parent in [link.clone(), link.join("child")] {
            spec.path = parent.join("installed");
            assert!(matches!(
                create_new_file(&spec, b"contents"),
                Err(FileError::SymlinkRejected { .. })
            ));
        }
        assert!(!real.join("installed").exists());
        assert!(!real.join("child/installed").exists());
        Ok(())
    }

    #[test]
    fn missing_parent_preserves_io_source() -> TestResult {
        let (directory, mut spec) = fixture()?;
        spec.path = directory.path().join("missing/installed");
        let Err(error) = create_new_file(&spec, b"contents") else {
            panic!("missing parent must reject file creation");
        };
        assert!(error.source().is_some(), "I/O source must be retained");
        assert!(matches!(
            error,
            FileError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound
        ));
        assert!(!directory.path().join("missing").exists());
        Ok(())
    }

    #[test]
    fn read_only_parent_leaves_no_temporary_file() -> TestResult {
        if rustix::process::geteuid().is_root() {
            tracing::info!("permission-denial fixture requires an unprivileged user");
            return Ok(());
        }
        let (directory, spec) = fixture()?;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500))
            .context(TestIoSnafu)?;
        let result = create_new_file(&spec, b"contents");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .context(TestIoSnafu)?;
        assert!(matches!(
            result,
            Err(FileError::Io { source, .. }) if source.kind() == io::ErrorKind::PermissionDenied
        ));
        assert_eq!(
            fs::read_dir(directory.path()).context(TestIoSnafu)?.count(),
            0
        );
        Ok(())
    }

    #[test]
    fn metadata_failure_cleans_up_temporary_file() -> TestResult {
        if rustix::process::geteuid().is_root() {
            tracing::info!("ownership-denial fixture requires an unprivileged user");
            return Ok(());
        }
        let (directory, mut spec) = fixture()?;
        spec.owner.uid = 0;
        assert!(matches!(
            create_new_file(&spec, b"contents"),
            Err(FileError::Io { .. })
        ));
        assert_eq!(
            fs::read_dir(directory.path()).context(TestIoSnafu)?.count(),
            0
        );
        Ok(())
    }

    #[test]
    fn removes_unchanged_created_file() -> TestResult {
        let (directory, spec) = fixture()?;
        let created = create_new_file(&spec, b"contents").context(TestFileSnafu)?;
        remove_created_file(&created).context(TestFileSnafu)?;
        assert_eq!(
            fs::read_dir(directory.path()).context(TestIoSnafu)?.count(),
            0
        );
        Ok(())
    }

    #[test]
    fn modified_file_is_not_removed() -> TestResult {
        let (_directory, spec) = fixture()?;
        let created = create_new_file(&spec, b"original").context(TestFileSnafu)?;
        fs::write(&spec.path, b"modified").context(TestIoSnafu)?;
        assert!(matches!(
            remove_created_file(&created),
            Err(FileError::ChangedSinceCreation { .. })
        ));
        assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, b"modified");
        Ok(())
    }

    #[test]
    fn replacement_with_identical_contents_is_not_removed() -> TestResult {
        let (directory, spec) = fixture()?;
        let created = create_new_file(&spec, b"original").context(TestFileSnafu)?;
        fs::rename(&spec.path, directory.path().join("saved")).context(TestIoSnafu)?;
        fs::write(&spec.path, b"original").context(TestIoSnafu)?;
        assert!(matches!(
            remove_created_file(&created),
            Err(FileError::ChangedSinceCreation { .. })
        ));
        assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, b"original");
        Ok(())
    }

    #[test]
    fn symlink_and_directory_replacements_are_not_removed() -> TestResult {
        for symlink in [false, true] {
            let (directory, spec) = fixture()?;
            let created = create_new_file(&spec, b"original").context(TestFileSnafu)?;
            let saved = directory.path().join("saved");
            fs::rename(&spec.path, &saved).context(TestIoSnafu)?;
            if symlink {
                std::os::unix::fs::symlink(&saved, &spec.path).context(TestIoSnafu)?;
            } else {
                fs::create_dir(&spec.path).context(TestIoSnafu)?;
            }
            assert!(matches!(
                remove_created_file(&created),
                Err(FileError::ChangedSinceCreation { .. })
            ));
            assert!(fs::symlink_metadata(&spec.path).is_ok());
        }
        Ok(())
    }

    #[test]
    fn creates_nested_directories_and_returns_rollback_order() -> TestResult {
        let (directory, _) = fixture()?;
        let first = directory.path().join("first");
        let second = first.join("second");
        let third = second.join("third");
        let paths = ensure_directory(&third, ownership(), 0o750).context(TestFileSnafu)?;
        assert_eq!(paths, [first, second, third.clone()]);
        for path in &paths {
            let metadata = fs::metadata(path).context(TestIoSnafu)?;
            assert_eq!(metadata.mode() & 0o7777, 0o750);
            assert_eq!(metadata.uid(), ownership().uid);
            assert_eq!(metadata.gid(), ownership().gid);
        }
        assert!(
            ensure_directory(&third, ownership(), 0o700)
                .context(TestFileSnafu)?
                .is_empty()
        );
        for path in paths.iter().rev() {
            assert_eq!(
                remove_created_directory(path).context(TestFileSnafu)?,
                DirectoryRemoval::Removed
            );
        }
        assert_eq!(
            fs::read_dir(directory.path()).context(TestIoSnafu)?.count(),
            0
        );
        Ok(())
    }

    #[test]
    fn existing_directory_metadata_is_unchanged() -> TestResult {
        let (directory, _) = fixture()?;
        let existing = directory.path().join("existing");
        fs::create_dir(&existing).context(TestIoSnafu)?;
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o711)).context(TestIoSnafu)?;
        let before = fs::metadata(&existing).context(TestIoSnafu)?;
        let child = existing.join("child");
        let paths = ensure_directory(&child, ownership(), 0o750).context(TestFileSnafu)?;
        assert_eq!(paths, [child]);
        let after = fs::metadata(&existing).context(TestIoSnafu)?;
        assert_eq!(before.mode(), after.mode());
        assert_eq!(before.uid(), after.uid());
        assert_eq!(before.gid(), after.gid());
        Ok(())
    }

    #[test]
    fn directory_creation_rejects_files_and_symlinks() -> TestResult {
        let (directory, spec) = fixture()?;
        fs::write(&spec.path, b"original").context(TestIoSnafu)?;
        for path in [spec.path.clone(), spec.path.join("child")] {
            assert!(matches!(
                ensure_directory(&path, ownership(), 0o700),
                Err(FileError::NotADirectory { .. })
            ));
        }
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(directory.path().join("missing"), &link).context(TestIoSnafu)?;
        assert!(matches!(
            ensure_directory(&link, ownership(), 0o700),
            Err(FileError::SymlinkRejected { .. })
        ));
        assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, b"original");
        Ok(())
    }

    #[test]
    fn nonempty_directory_is_not_removed() -> TestResult {
        let (directory, spec) = fixture()?;
        fs::write(&spec.path, b"original").context(TestIoSnafu)?;
        assert_eq!(
            remove_created_directory(directory.path()).context(TestFileSnafu)?,
            DirectoryRemoval::KeptNotEmpty
        );
        assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, b"original");
        Ok(())
    }

    #[test]
    fn publishing_and_link_fallback_never_overwrite() -> TestResult {
        let (directory, spec) = fixture()?;
        let (parent, name) = parent_directory(&spec.path).context(TestFileSnafu)?;
        let temporary = directory.path().join("temporary");
        fs::write(&temporary, b"new").context(TestIoSnafu)?;
        fs::write(&spec.path, b"original").context(TestIoSnafu)?;
        for publish in [publish_file, link_file] {
            assert!(matches!(
                publish(&parent, Path::new("temporary"), name, &spec.path),
                Err(FileError::AlreadyExists { .. })
            ));
            assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, b"original");
            assert_eq!(fs::read(&temporary).context(TestIoSnafu)?, b"new");
        }
        fs::remove_file(&spec.path).context(TestIoSnafu)?;
        link_file(&parent, Path::new("temporary"), name, &spec.path).context(TestFileSnafu)?;
        assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, b"new");
        assert!(!temporary.exists());
        Ok(())
    }

    #[test]
    fn secrets_are_absent_from_errors_and_debug() -> TestResult {
        let (_directory, spec) = fixture()?;
        let secret = b"never-display-this-secret-content";
        let created = create_new_file(&spec, secret).context(TestFileSnafu)?;
        let Err(secret_error) = create_new_file(&spec, secret) else {
            panic!("existing destination must reject secret contents");
        };
        let diagnostics = format!("{spec:?} {created:?} {secret_error} {secret_error:?}");
        assert!(!diagnostics.contains("never-display-this-secret-content"));
        assert!(!diagnostics.contains(&format!("{secret:?}")));
        assert!(
            secret_error
                .to_string()
                .contains(&spec.path.display().to_string())
        );
        Ok(())
    }
}
