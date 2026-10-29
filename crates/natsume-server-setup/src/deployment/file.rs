//! Atomic deployment file operations for first-install setup.

use std::{
    fs::File,
    io::{self, Read as _, Write as _},
    os::unix::fs::MetadataExt as _,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
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
    /// The rollback directory and target are on different filesystems.
    #[snafu(display("rollback filesystem differs between {} and {}", path.display(), quarantine_path.display()))]
    RollbackFilesystem {
        /// Original target left unchanged.
        path: PathBuf,
        /// Rejected quarantine directory.
        quarantine_path: PathBuf,
    },
    /// Publication succeeded but synchronizing the parent directory failed.
    #[snafu(display("published file is not durable: {}", created.path.display()))]
    PublishedNotDurable {
        /// Published object that must be included in rollback.
        created: CreatedFile,
        /// Original directory persistence failure.
        source: io::Error,
    },
    /// A rejected rollback object could not be restored to its original name.
    #[snafu(display("rollback object retained at {}; cannot restore {}: {validation}", isolated_path.display(), path.display()))]
    RestoreRollback {
        /// Original path, which was not overwritten.
        path: PathBuf,
        /// Quarantined object retained for administrator recovery.
        isolated_path: PathBuf,
        /// Original validation failure.
        validation: Box<FileError>,
        /// Original no-replace restoration failure.
        source: io::Error,
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
    /// A managed directory has unexpected ownership or permissions.
    #[snafu(display("directory metadata rejected for {}: expected {}:{} {:04o}, actual {}:{} {:04o}", path.display(), expected_owner.uid, expected_owner.gid, expected_mode, actual_owner.uid, actual_owner.gid, actual_mode))]
    DirectoryMetadata {
        /// Directory whose metadata was rejected.
        path: PathBuf,
        /// Required owner and group.
        expected_owner: Ownership,
        /// Required permission bits, including special bits.
        expected_mode: u32,
        /// Observed owner and group.
        actual_owner: Ownership,
        /// Observed permission bits, including special bits.
        actual_mode: u32,
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
/// Existing directories must match the requested ownership and exact permission bits.
///
/// # Errors
/// Rejects symbolic links, non-directory components, and filesystem failures.
pub fn ensure_directory(
    path: &Path,
    owner: Ownership,
    mode: u32,
) -> Result<Vec<PathBuf>, FileError> {
    let (directory, created) = walk_directory(path, Some((owner, mode)), None)?;
    validate_directory_metadata(
        &directory.metadata().context(IoSnafu { path })?,
        path,
        owner,
        mode,
    )?;
    Ok(created)
}

fn walk_directory(
    path: &Path,
    create: Option<(Ownership, u32)>,
    protected_owner: Option<Ownership>,
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
    if let Some(owner) = protected_owner {
        validate_protected_directory(&directory, &current, owner)?;
    }
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
        if let Some(owner) = protected_owner {
            validate_protected_directory(&child, &current, owner)?;
        }
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
    let (directory, _) = walk_directory(parent, None, None)?;
    Ok((directory, Path::new(name)))
}

/// Installs contents as a new file without ever including the supplied bytes in diagnostics.
///
/// Publication disarms temporary-name cleanup immediately. The hard-link fallback checks
/// the temporary name's device/inode against its held fd before unlinking that name;
/// a replacement is preserved and is never retried by the destructor.
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
                armed: true,
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
    publish_file(&directory, &temporary.name, name, path, &temporary.file)?;
    temporary.armed = false;
    #[cfg(test)]
    tests::after_publication(&parent.join(&temporary.name)).context(IoSnafu { path })?;
    let device = metadata.dev();
    #[cfg(test)]
    let device = tests::file_device(path, device);
    let created = CreatedFile {
        path: path.clone(),
        device,
        inode: metadata.ino(),
        sha256: Sha256::digest(contents).into(),
    };
    drop(temporary);
    sync_publication(&directory).context(PublishedNotDurableSnafu {
        created: created.clone(),
    })?;
    Ok(created)
}

/// Publishes a file and records any published object even when durability fails.
///
/// # Errors
/// Returns the original publication failure after retaining any published creation record.
pub(crate) fn create_recorded_file(
    spec: &FileSpec,
    contents: &[u8],
    files: &mut Vec<CreatedFile>,
) -> Result<(), FileError> {
    match create_new_file(spec, contents) {
        Ok(created) => {
            files.push(created);
            Ok(())
        }
        Err(error) => {
            if let FileError::PublishedNotDurable { created, .. } = &error {
                files.push(created.clone());
            }
            Err(error)
        }
    }
}

fn sync_publication(directory: &File) -> io::Result<()> {
    #[cfg(test)]
    tests::check_publication_sync()?;
    directory.sync_all()
}

struct TemporaryFile<'directory> {
    file: File,
    directory: &'directory File,
    name: PathBuf,
    armed: bool,
}

impl Drop for TemporaryFile<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Err(source) = unlink_temporary(self.directory, &self.name, &self.file) {
            tracing::warn!(name = ?self.name, %source, "failed to remove deployment temporary file");
        }
    }
}

fn unlink_temporary(directory: &File, name: &Path, held: &File) -> io::Result<()> {
    let metadata = held.metadata()?;
    match unix_fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) if stat.st_dev == metadata.dev() && stat.st_ino == metadata.ino() => {
            unix_fs::unlinkat(directory, name, AtFlags::empty())?;
            Ok(())
        }
        Ok(_) | Err(rustix::io::Errno::NOENT) => Ok(()),
        Err(source) => Err(source.into()),
    }
}

fn publish_file(
    directory: &File,
    temporary: &Path,
    name: &Path,
    path: &Path,
    held: &File,
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
            link_file(directory, temporary, name, path, held)
        }
        Err(source) => Err(io::Error::from(source)).context(IoSnafu { path }),
    }
}

fn link_file(
    directory: &File,
    temporary: &Path,
    name: &Path,
    path: &Path,
    held: &File,
) -> Result<(), FileError> {
    match unix_fs::linkat(directory, temporary, directory, name, AtFlags::empty()) {
        Ok(()) => {}
        Err(rustix::io::Errno::EXIST) => return AlreadyExistsSnafu { path }.fail(),
        Err(source) => return Err(io::Error::from(source)).context(IoSnafu { path }),
    }
    #[cfg(test)]
    tests::after_link(temporary).context(IoSnafu { path })?;
    if let Err(source) = unlink_temporary(directory, temporary, held) {
        tracing::warn!(%source, "cannot remove published temporary link");
    }
    Ok(())
}

fn validate_protected_directory(
    directory: &File,
    path: &Path,
    owner: Ownership,
) -> Result<(), FileError> {
    let metadata = directory.metadata().context(IoSnafu { path })?;
    let trusted_owner = metadata.uid() == owner.uid || metadata.uid() == ROOT_OWNER.uid;
    let sticky_root = metadata.uid() == ROOT_OWNER.uid && metadata.mode() & 0o1000 != 0;
    ensure!(
        trusted_owner && (metadata.mode() & 0o022 == 0 || sticky_root),
        MetadataSnafu { path }
    );
    Ok(())
}

fn protected_directory(path: &Path, owner: Ownership) -> Result<File, FileError> {
    ensure!(
        path.is_absolute()
            && !path
                .components()
                .any(|part| matches!(part, Component::ParentDir)),
        InvalidPathSnafu { path }
    );
    let (directory, _) = walk_directory(path, None, Some(owner))?;
    ensure!(
        directory.metadata().context(IoSnafu { path })?.mode() & 0o022 == 0,
        MetadataSnafu { path }
    );
    Ok(directory)
}

/// Root-private rollback storage reached through an administrator-controlled parent chain.
#[derive(Debug)]
pub(super) struct RollbackDirectory {
    directory: File,
    parent: File,
    name: PathBuf,
    path: PathBuf,
    created: AtomicBool,
}

impl RollbackDirectory {
    /// Opens or creates a 0700 quarantine without traversing service-writable ancestors.
    ///
    /// # Errors
    /// Rejects unsafe ancestors, unexpected quarantine metadata, and filesystem failures.
    pub(super) fn open(path: &Path, owner: Ownership) -> Result<Self, FileError> {
        let parent_path = path.parent().context(InvalidPathSnafu { path })?;
        let name = path.file_name().context(InvalidPathSnafu { path })?;
        let parent = protected_directory(parent_path, owner)?;
        let created = match unix_fs::mkdirat(&parent, name, Mode::RWXU) {
            Ok(()) => true,
            Err(rustix::io::Errno::EXIST) => false,
            Err(source) => return Err(io::Error::from(source)).context(IoSnafu { path }),
        };
        let directory = unix_fs::openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(io::Error::from)
        .context(IoSnafu { path })?;
        if created {
            set_metadata(&directory, path, owner, 0o700)?;
        }
        validate_directory_metadata(
            &directory.metadata().context(IoSnafu { path })?,
            path,
            owner,
            0o700,
        )?;
        Ok(Self {
            directory,
            parent,
            name: name.into(),
            path: path.into(),
            created: AtomicBool::new(created),
        })
    }

    /// Checks the keys filesystem before any deployment file is published.
    ///
    /// # Errors
    /// Returns a named filesystem mismatch or the original inspection failure.
    pub(super) fn check_filesystem(&self, keys: &Path) -> Result<(), FileError> {
        let target = existing_directory_metadata(keys)?;
        let device = self
            .directory
            .metadata()
            .context(IoSnafu { path: &self.path })?
            .dev();
        #[cfg(test)]
        let device = tests::rollback_device(device);
        ensure!(
            target.dev() == device,
            RollbackFilesystemSnafu {
                path: keys,
                quarantine_path: &self.path
            }
        );
        Ok(())
    }

    /// Removes this attempt's empty quarantine after rollback, preserving retained objects.
    pub(super) fn cleanup(&self) {
        if self.created.load(Ordering::Relaxed) {
            match unix_fs::unlinkat(&self.parent, &self.name, AtFlags::REMOVEDIR) {
                Ok(()) | Err(rustix::io::Errno::NOENT) => {
                    self.created.store(false, Ordering::Relaxed);
                }
                Err(rustix::io::Errno::NOTEMPTY) => {}
                Err(source) => {
                    tracing::error!(path = ?self.path, %source, "cannot remove rollback directory");
                }
            }
        }
    }

    /// Selects protected-directory unlink or service-writable quarantine using actual metadata.
    ///
    /// # Errors
    /// Returns the original validation, isolation, or removal error.
    pub(super) fn remove_deployment_file(&self, created: &CreatedFile) -> Result<(), FileError> {
        let (directory, _) = parent_directory(&created.path)?;
        if administrator_directory(&directory, &created.path)? {
            remove_created_file(created)
        } else {
            self.remove(created)
        }
    }

    /// Isolates and removes an unchanged file on the quarantine's filesystem.
    ///
    /// # Errors
    /// Rejects cross-filesystem moves and changed files; preserves failed restorations for recovery.
    pub(super) fn remove(&self, created: &CreatedFile) -> Result<(), FileError> {
        let (directory, name) = parent_directory(&created.path)?;
        let target_device = directory
            .metadata()
            .context(IoSnafu {
                path: &created.path,
            })?
            .dev();
        let quarantine_device = self
            .directory
            .metadata()
            .context(IoSnafu { path: &self.path })?
            .dev();
        #[cfg(test)]
        let quarantine_device = tests::rollback_device(quarantine_device);
        ensure_same_filesystem(created, target_device, quarantine_device, &self.path)?;
        remove_isolated_file(created, &directory, name, &self.directory, &self.path)
    }
}

impl Drop for RollbackDirectory {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn existing_directory_metadata(mut path: &Path) -> Result<std::fs::Metadata, FileError> {
    loop {
        if let Some(metadata) = directory_metadata(path)? {
            return Ok(metadata);
        }
        path = path.parent().context(InvalidPathSnafu { path })?;
    }
}

/// Validates rollback metadata and mount compatibility without creating any entries.
///
/// # Errors
/// Rejects unsafe ancestry, existing quarantine metadata, and differing keys filesystems.
pub(super) fn check_rollback_directory(spec: &FileSpec, keys: &Path) -> Result<(), FileError> {
    let parent = spec
        .path
        .parent()
        .context(InvalidPathSnafu { path: &spec.path })?;
    let directory = protected_directory(parent, spec.owner)?;
    let quarantine = directory_metadata(&spec.path)?;
    if let Some(metadata) = &quarantine {
        validate_directory_metadata(metadata, &spec.path, spec.owner, 0o700)?;
    }
    let device = match quarantine {
        Some(metadata) => metadata.dev(),
        None => directory
            .metadata()
            .context(IoSnafu { path: parent })?
            .dev(),
    };
    ensure!(
        existing_directory_metadata(keys)?.dev() == device,
        RollbackFilesystemSnafu {
            path: keys,
            quarantine_path: &spec.path
        }
    );
    Ok(())
}

fn ensure_same_filesystem(
    created: &CreatedFile,
    target_device: u64,
    quarantine_device: u64,
    quarantine_path: &Path,
) -> Result<(), FileError> {
    ensure!(
        target_device == quarantine_device && created.device == quarantine_device,
        RollbackFilesystemSnafu {
            path: &created.path,
            quarantine_path
        }
    );
    Ok(())
}

/// Validates and unlinks an unchanged file in an administrator-controlled directory.
///
/// The parent must be root-owned without group/other write permission, so a service
/// user cannot replace its directory entry between fd identity/hash validation and unlinkat.
/// No rename or quarantine allocation is needed for protected deployment or PKI files.
///
/// # Errors
/// Rejects writable ancestors, non-administrator parents, changed files, and filesystem errors.
pub fn remove_created_file(created: &CreatedFile) -> Result<(), FileError> {
    let path = &created.path;
    let parent = path.parent().context(InvalidPathSnafu { path })?;
    let name = path.file_name().context(InvalidPathSnafu { path })?;
    let owner = Ownership {
        uid: rustix::process::geteuid().as_raw(),
        gid: rustix::process::getegid().as_raw(),
    };
    let directory = protected_directory(parent, owner)?;
    ensure!(
        administrator_directory(&directory, parent)?,
        MetadataSnafu { path: parent }
    );
    #[cfg(test)]
    tests::before_protected_removal(path).context(IoSnafu { path })?;
    validate_created_file(&directory, Path::new(name), created)?;
    unix_fs::unlinkat(&directory, name, AtFlags::empty())
        .map_err(io::Error::from)
        .context(IoSnafu { path })?;
    directory.sync_all().context(IoSnafu { path })
}

fn administrator_directory(directory: &File, path: &Path) -> Result<bool, FileError> {
    let metadata = directory.metadata().context(IoSnafu { path })?;
    let administrator = ROOT_OWNER.uid;
    #[cfg(test)]
    let administrator = tests::administrator_uid(administrator);
    Ok(metadata.uid() == administrator && metadata.mode() & 0o022 == 0)
}

fn remove_isolated_file(
    created: &CreatedFile,
    directory: &File,
    name: &Path,
    quarantine: &File,
    quarantine_path: &Path,
) -> Result<(), FileError> {
    let path = &created.path;
    let isolated = tempfile::Builder::new()
        .prefix(".natsume-setup-rollback-")
        .disable_cleanup(true)
        .make_in(quarantine_path, |candidate| {
            let isolated_name = candidate.file_name().ok_or(io::ErrorKind::InvalidInput)?;
            unix_fs::renameat_with(
                directory,
                name,
                quarantine,
                isolated_name,
                RenameFlags::NOREPLACE,
            )?;
            Ok(PathBuf::from(isolated_name))
        })
        .context(IoSnafu { path })?
        .into_parts()
        .0;
    #[cfg(test)]
    tests::after_isolation(&quarantine_path.join(&isolated)).context(IoSnafu { path })?;
    if let Err(validation) = validate_created_file(quarantine, &isolated, created) {
        if let Err(source) = unix_fs::renameat_with(
            quarantine,
            &isolated,
            directory,
            name,
            RenameFlags::NOREPLACE,
        ) {
            let isolated_path = quarantine_path.join(&isolated);
            tracing::error!(?path, ?isolated_path, %source, "cannot restore rollback object; retained in quarantine");
            return Err(io::Error::from(source)).context(RestoreRollbackSnafu {
                path,
                isolated_path,
                validation: Box::new(validation),
            });
        }
        return Err(validation);
    }
    #[cfg(test)]
    tests::after_validation(&quarantine_path.join(&isolated)).context(IoSnafu { path })?;
    unix_fs::unlinkat(quarantine, &isolated, AtFlags::empty())
        .map_err(io::Error::from)
        .context(IoSnafu { path })?;
    quarantine.sync_all().context(IoSnafu {
        path: quarantine_path,
    })?;
    directory.sync_all().context(IoSnafu { path })
}

fn validate_created_file(
    directory: &File,
    name: &Path,
    created: &CreatedFile,
) -> Result<(), FileError> {
    let path = &created.path;
    let before = unix_fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(io::Error::from)
        .context(IoSnafu { path })?;
    let device = before.st_dev;
    #[cfg(test)]
    let device = tests::file_device(path, device);
    ensure!(
        FileType::from_raw_mode(before.st_mode) == FileType::RegularFile
            && device == created.device
            && before.st_ino == created.inode,
        ChangedSinceCreationSnafu { path }
    );
    let mut file = File::from(
        unix_fs::openat(
            directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)
        .context(IoSnafu { path })?,
    );
    let metadata = file.metadata().context(IoSnafu { path })?;
    let device = metadata.dev();
    #[cfg(test)]
    let device = tests::file_device(path, device);
    ensure!(
        metadata.is_file() && device == created.device && metadata.ino() == created.inode,
        ChangedSinceCreationSnafu { path }
    );
    let mut hash = Sha256::new();
    let mut buffer = Zeroizing::new(vec![0; 8192]);
    loop {
        let read = file.read(&mut buffer).context(IoSnafu { path })?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    let digest: [u8; 32] = hash.finalize().into();
    ensure!(digest == created.sha256, ChangedSinceCreationSnafu { path });
    Ok(())
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
    walk_directory(path, None, None).map(|(directory, _)| directory)
}

/// Verifies ownership and exact permissions through a symlink-resistant directory walk.
///
/// # Errors
/// Rejects parent traversal, symbolic links, unsafe metadata, and filesystem failures.
pub(crate) fn check_directory(path: &Path, owner: Ownership, mode: u32) -> Result<(), FileError> {
    let metadata = open_directory(path)?.metadata().context(IoSnafu { path })?;
    validate_directory_metadata(&metadata, path, owner, mode)
}

fn validate_directory_metadata(
    metadata: &std::fs::Metadata,
    path: &Path,
    owner: Ownership,
    mode: u32,
) -> Result<(), FileError> {
    let actual_owner = Ownership {
        uid: metadata.uid(),
        gid: metadata.gid(),
    };
    let actual_mode = metadata.mode() & 0o7777;
    ensure!(
        actual_owner == owner && actual_mode == mode,
        DirectoryMetadataSnafu {
            path,
            expected_owner: owner,
            expected_mode: mode,
            actual_owner,
            actual_mode,
        }
    );
    Ok(())
}

/// Inspects a directory through no-follow handles without creating missing components.
///
/// # Errors
/// Rejects invalid paths, symbolic links, non-directories, and filesystem failures.
pub(super) fn directory_metadata(path: &Path) -> Result<Option<std::fs::Metadata>, FileError> {
    match open_directory(path) {
        Ok(directory) => directory.metadata().map(Some).context(IoSnafu { path }),
        Err(FileError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
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
pub(crate) mod tests {
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

    type TemporaryHook = Box<dyn FnOnce(&Path) -> io::Result<()>>;

    thread_local! {
        static AFTER_PUBLICATION: std::cell::RefCell<Option<TemporaryHook>> = const { std::cell::RefCell::new(None) };
        static AFTER_LINK: std::cell::RefCell<Option<TemporaryHook>> = const { std::cell::RefCell::new(None) };
    }

    /// Runs the one-shot old-temporary-name replacement fixture.
    pub(super) fn after_publication(path: &Path) -> io::Result<()> {
        if let Some(hook) = AFTER_PUBLICATION.take() {
            hook(path)?;
        }
        Ok(())
    }

    /// Runs the one-shot hard-link replacement fixture.
    pub(super) fn after_link(path: &Path) -> io::Result<()> {
        if let Some(hook) = AFTER_LINK.take() {
            hook(path)?;
        }
        Ok(())
    }

    #[test]
    fn published_temporary_name_replacement_survives_guard_drop() -> TestResult {
        let (root, spec) = fixture()?;
        let replacement = root.path().join("replacement-name");
        let observed = replacement.clone();
        AFTER_PUBLICATION.set(Some(Box::new(move |path| {
            fs::write(path, b"replacement")?;
            fs::hard_link(path, observed)
        })));
        create_new_file(&spec, b"installed").context(TestFileSnafu)?;
        assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, b"installed");
        assert_eq!(
            fs::metadata(replacement).context(TestIoSnafu)?.nlink(),
            2,
            "Drop must preserve the replacement at the old temporary name"
        );
        Ok(())
    }

    #[test]
    fn link_fallback_preserves_replaced_temporary_name() -> TestResult {
        let (root, spec) = fixture()?;
        let temporary = root.path().join("temporary");
        fs::write(&temporary, b"installed").context(TestIoSnafu)?;
        let held = File::open(&temporary).context(TestIoSnafu)?;
        let replacement = temporary.clone();
        AFTER_LINK.set(Some(Box::new(move |_| {
            fs::remove_file(&replacement)?;
            fs::write(&replacement, b"replacement")
        })));
        let (parent, name) = parent_directory(&spec.path).context(TestFileSnafu)?;
        link_file(&parent, Path::new("temporary"), name, &spec.path, &held)
            .context(TestFileSnafu)?;
        assert_eq!(fs::read(temporary).context(TestIoSnafu)?, b"replacement");
        assert_eq!(fs::read(spec.path).context(TestIoSnafu)?, b"installed");
        Ok(())
    }

    thread_local! {
        static PUBLICATION_SYNCS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    }

    /// Resets this thread's injected persistence failure when the fixture ends.
    pub(crate) struct PublicationSyncFault;

    impl Drop for PublicationSyncFault {
        fn drop(&mut self) {
            PUBLICATION_SYNCS.set(None);
        }
    }

    /// Fails one publication after the requested number of successful directory syncs.
    pub(crate) fn fail_publication_sync_after(successes: usize) -> PublicationSyncFault {
        PUBLICATION_SYNCS.set(Some(successes));
        PublicationSyncFault
    }

    /// Consumes this thread's configured persistence fault.
    pub(super) fn check_publication_sync() -> io::Result<()> {
        interrupt_publication();
        PUBLICATION_SYNCS.with(|remaining| match remaining.get() {
            Some(0) => {
                remaining.set(None);
                Err(io::Error::other("injected directory fsync failure"))
            }
            Some(count) => {
                remaining.set(Some(count - 1));
                Ok(())
            }
            None => Ok(()),
        })
    }

    thread_local! {
        static PUBLICATION_INTERRUPT: std::cell::RefCell<Option<(usize, std::sync::Arc<std::sync::atomic::AtomicBool>)>> = const { std::cell::RefCell::new(None) };
    }

    /// Resets this thread's pending-publication interrupt fixture.
    pub(crate) struct PublicationInterrupt;

    impl Drop for PublicationInterrupt {
        fn drop(&mut self) {
            PUBLICATION_INTERRUPT.set(None);
        }
    }

    /// Interrupts after a selected pending file is published, before its directory sync.
    pub(crate) fn interrupt_publication_after(
        successes: usize,
        flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> PublicationInterrupt {
        PUBLICATION_INTERRUPT.set(Some((successes, flag)));
        PublicationInterrupt
    }

    fn interrupt_publication() {
        PUBLICATION_INTERRUPT.with_borrow_mut(|state| {
            if let Some((remaining, flag)) = state {
                if *remaining == 0 {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    *state = None;
                } else {
                    *remaining -= 1;
                }
            }
        });
    }

    type RollbackHook = Box<dyn FnOnce(&Path) -> io::Result<()>>;

    thread_local! {
        static ROLLBACK_DEVICE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
        static BEFORE_PROTECTED_REMOVAL: std::cell::RefCell<Option<RollbackHook>> = const { std::cell::RefCell::new(None) };
        static AFTER_ISOLATION: std::cell::RefCell<Option<RollbackHook>> = const { std::cell::RefCell::new(None) };
        static AFTER_VALIDATION: std::cell::RefCell<Option<RollbackHook>> = const { std::cell::RefCell::new(None) };
    }

    /// Modifies the next protected rollback target to produce a visible partial cleanup failure.
    pub(crate) fn modify_next_rollback_file() {
        BEFORE_PROTECTED_REMOVAL.set(Some(Box::new(|path| {
            fs::write(path, b"modified during rollback")
        })));
    }

    /// Runs the one-shot protected rollback fault.
    pub(super) fn before_protected_removal(path: &Path) -> io::Result<()> {
        if let Some(hook) = BEFORE_PROTECTED_REMOVAL.take() {
            hook(path)?;
        }
        Ok(())
    }

    thread_local! {
        static SEPARATE_DEVICE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    }

    /// Restores actual device IDs when a separate-mount fixture ends.
    pub(crate) struct SeparateDevice;

    impl Drop for SeparateDevice {
        fn drop(&mut self) {
            SEPARATE_DEVICE.set(None);
        }
    }

    /// Simulates a separate filesystem for file identities below the supplied directory.
    pub(crate) fn separate_device(path: PathBuf) -> SeparateDevice {
        SEPARATE_DEVICE.set(Some(path));
        SeparateDevice
    }

    /// Supplies consistent simulated device IDs to publication and fd identity validation.
    pub(super) fn file_device(path: &Path, actual: u64) -> u64 {
        SEPARATE_DEVICE.with_borrow(|root| {
            if root.as_ref().is_some_and(|root| path.starts_with(root)) {
                actual ^ 1
            } else {
                actual
            }
        })
    }

    /// Uses the fixture administrator identity for non-root tests.
    pub(super) fn administrator_uid(_root: u32) -> u32 {
        rustix::process::geteuid().as_raw()
    }

    /// Overrides the next quarantine device inspection.
    pub(crate) fn mismatch_rollback_device(device: u64) {
        ROLLBACK_DEVICE.set(Some(device ^ 1));
    }

    /// Consumes a one-shot filesystem-device override.
    pub(super) fn rollback_device(actual: u64) -> u64 {
        ROLLBACK_DEVICE.take().unwrap_or(actual)
    }

    /// Runs the one-shot restoration-conflict fixture.
    pub(super) fn after_isolation(path: &Path) -> io::Result<()> {
        if let Some(hook) = AFTER_ISOLATION.take() {
            hook(path)?;
        }
        Ok(())
    }

    /// Runs the one-shot service-directory replacement fixture.
    pub(super) fn after_validation(path: &Path) -> io::Result<()> {
        if let Some(hook) = AFTER_VALIDATION.take() {
            hook(path)?;
        }
        Ok(())
    }

    #[test]
    fn quarantine_protects_validated_name_and_preserves_service_replacement() -> TestResult {
        let (root, mut spec) = fixture()?;
        let service = root.path().join("service");
        fs::create_dir(&service).context(TestIoSnafu)?;
        fs::set_permissions(&service, fs::Permissions::from_mode(0o777)).context(TestIoSnafu)?;
        spec.path = service.join("installed");
        let created = create_new_file(&spec, b"original").context(TestFileSnafu)?;
        let path = root.path().join("rollback");
        let quarantine = RollbackDirectory::open(&path, ownership()).context(TestFileSnafu)?;
        let replacement = spec.path.clone();
        let expected_parent = path.clone();
        AFTER_VALIDATION.set(Some(Box::new(move |isolated| {
            assert_eq!(isolated.parent(), Some(expected_parent.as_path()));
            let metadata = fs::metadata(&expected_parent)?;
            assert_eq!(metadata.mode() & 0o7777, 0o700);
            assert_eq!(
                (metadata.uid(), metadata.gid()),
                (ownership().uid, ownership().gid)
            );
            assert_eq!(metadata.dev(), created.device);
            fs::write(replacement, b"service replacement")?;
            Ok(())
        })));
        quarantine.remove(&created).context(TestFileSnafu)?;
        assert_eq!(
            fs::read(&spec.path).context(TestIoSnafu)?,
            b"service replacement"
        );
        assert_eq!(fs::read_dir(&path).context(TestIoSnafu)?.count(), 0);
        drop(quarantine);
        assert!(
            !path.exists(),
            "empty quarantine created by this rollback must be removed"
        );
        Ok(())
    }

    #[test]
    fn quarantine_rejects_cross_filesystem_before_moving_target() -> TestResult {
        let (root, spec) = fixture()?;
        let created = create_new_file(&spec, b"original").context(TestFileSnafu)?;
        let path = root.path().join("rollback");
        let quarantine = RollbackDirectory::open(&path, ownership()).context(TestFileSnafu)?;
        ROLLBACK_DEVICE.set(Some(created.device ^ 1));
        assert!(matches!(
            quarantine.remove(&created),
            Err(FileError::RollbackFilesystem { .. })
        ));
        assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, b"original");
        assert_eq!(fs::read_dir(&path).context(TestIoSnafu)?.count(), 0);
        drop(quarantine);
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn quarantine_restores_modified_file_and_retains_failed_restoration() -> TestResult {
        for conflict in [false, true] {
            let (root, spec) = fixture()?;
            let created = create_new_file(&spec, b"original").context(TestFileSnafu)?;
            fs::write(&spec.path, b"modified").context(TestIoSnafu)?;
            let path = root.path().join("rollback");
            let quarantine = RollbackDirectory::open(&path, ownership()).context(TestFileSnafu)?;
            if conflict {
                let original = spec.path.clone();
                AFTER_ISOLATION.set(Some(Box::new(move |_| {
                    fs::write(original, b"service replacement")
                })));
            }
            let Err(error) = quarantine.remove(&created) else {
                panic!("modified file must survive rollback");
            };
            if conflict {
                let FileError::RestoreRollback {
                    path: original,
                    isolated_path,
                    validation,
                    source,
                } = &error
                else {
                    panic!("restoration conflict must report both paths: {error:?}");
                };
                assert_eq!(original, &spec.path);
                assert_eq!(isolated_path.parent(), Some(path.as_path()));
                assert!(matches!(
                    **validation,
                    FileError::ChangedSinceCreation { .. }
                ));
                assert_eq!(source.kind(), io::ErrorKind::AlreadyExists);
                assert_eq!(fs::read(isolated_path).context(TestIoSnafu)?, b"modified");
                assert_eq!(
                    fs::read(original).context(TestIoSnafu)?,
                    b"service replacement"
                );
                let message = error.to_string();
                assert!(message.contains(&original.display().to_string()));
                assert!(message.contains(&isolated_path.display().to_string()));
            } else {
                assert!(matches!(error, FileError::ChangedSinceCreation { .. }));
                assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, b"modified");
            }
            drop(quarantine);
            assert_eq!(path.exists(), conflict, "preserve only nonempty quarantine");
        }
        Ok(())
    }

    #[test]
    fn quarantine_rejects_writable_ancestors_and_loose_metadata() -> TestResult {
        let (root, _) = fixture()?;
        let parent = root.path().join("service-owned");
        fs::create_dir(&parent).context(TestIoSnafu)?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).context(TestIoSnafu)?;
        let path = parent.join("rollback");
        assert!(matches!(
            RollbackDirectory::open(&path, ownership()),
            Err(FileError::Metadata { .. })
        ));
        assert!(!path.exists());
        let path = root.path().join("rollback");
        fs::create_dir(&path).context(TestIoSnafu)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).context(TestIoSnafu)?;
        assert!(matches!(
            RollbackDirectory::open(&path, ownership()),
            Err(FileError::DirectoryMetadata { .. })
        ));
        let quarantine = RollbackDirectory::open(&parent.join("rollback"), ROOT_OWNER);
        assert!(
            quarantine.is_err(),
            "service-owned or writable ancestor must be rejected"
        );
        Ok(())
    }

    #[test]
    fn published_sync_failure_retains_rollback_record() -> TestResult {
        let (directory, spec) = fixture()?;
        let _fault = fail_publication_sync_after(0);
        let mut files = Vec::new();
        let Err(error) = create_recorded_file(&spec, b"contents", &mut files) else {
            panic!("injected fsync must fail")
        };
        assert!(matches!(error, FileError::PublishedNotDurable { .. }));
        assert!(error.source().is_some());
        assert_eq!(files.len(), 1);
        assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, b"contents");
        cleanup(&files, &[]);
        assert_eq!(
            fs::read_dir(directory.path()).context(TestIoSnafu)?.count(),
            0
        );
        Ok(())
    }

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
            Err(FileError::DirectoryMetadata { .. })
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
            ensure_directory(&third, ownership(), 0o750)
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
        let held = File::open(&temporary).context(TestIoSnafu)?;
        for publish in [publish_file, link_file] {
            assert!(matches!(
                publish(&parent, Path::new("temporary"), name, &spec.path, &held),
                Err(FileError::AlreadyExists { .. })
            ));
            assert_eq!(fs::read(&spec.path).context(TestIoSnafu)?, b"original");
            assert_eq!(fs::read(&temporary).context(TestIoSnafu)?, b"new");
        }
        fs::remove_file(&spec.path).context(TestIoSnafu)?;
        link_file(&parent, Path::new("temporary"), name, &spec.path, &held)
            .context(TestFileSnafu)?;
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
