use std::{
    fs::{self, File},
    io::{self, Write as _},
    os::unix::fs::PermissionsExt as _,
    path::Path,
};

use snafu::Snafu;
use tempfile::Builder;

pub(crate) const ATOMIC_TEMP_PREFIX: &str = ".natsume-tmp";

#[derive(Clone, Copy)]
pub(super) enum WritePolicy {
    CreateOnly,
    Replace,
}

#[derive(Debug, Snafu)]
pub(super) enum AtomicWriteError {
    #[snafu(display("atomic file target already exists"))]
    Conflict,

    #[snafu(display("atomic file persistence failed"))]
    Failed,
}

/// Writes one complete file through a same-directory temporary file, fsync, rename, and directory
/// fsync. Mode is the only policy input; ownership remains that of the calling service user.
pub(super) fn atomic_write(
    target: &Path,
    content: &[u8],
    mode: u32,
    policy: WritePolicy,
) -> Result<(), AtomicWriteError> {
    let parent = target.parent().ok_or(AtomicWriteError::Failed)?;
    let mut temporary = Builder::new()
        .prefix(ATOMIC_TEMP_PREFIX)
        .tempfile_in(parent)
        .map_err(|_| AtomicWriteError::Failed)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|_| AtomicWriteError::Failed)?;
    temporary
        .write_all(content)
        .map_err(|_| AtomicWriteError::Failed)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| AtomicWriteError::Failed)?;
    match policy {
        WritePolicy::CreateOnly => match rustix::fs::renameat_with(
            rustix::fs::CWD,
            temporary.path(),
            rustix::fs::CWD,
            target,
            rustix::fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) => return Err(AtomicWriteError::Conflict),
            Err(_) => return Err(AtomicWriteError::Failed),
        },
        WritePolicy::Replace => {
            fs::rename(temporary.path(), target).map_err(|_| AtomicWriteError::Failed)?;
        }
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| AtomicWriteError::Failed)
}

/// Removes a file if present and makes the removed directory entry durable before returning.
pub(super) fn durable_remove(target: &Path) -> io::Result<()> {
    match fs::remove_file(target) {
        Ok(()) => {
            let parent = target.parent().ok_or(io::ErrorKind::InvalidInput)?;
            File::open(parent)?.sync_all()
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::MetadataExt as _};

    use tempfile::TempDir;

    use super::*;

    fn tempdir() -> TempDir {
        match TempDir::new() {
            Ok(directory) => directory,
            Err(error) => panic!("test directory must be created: {error}"),
        }
    }

    #[test]
    fn new_file_is_complete_with_the_requested_mode() {
        let directory = tempdir();
        let target = directory.path().join("record.json");
        let new = vec![b'n'; 64 * 1024];

        if let Err(error) = atomic_write(&target, &new, 0o600, WritePolicy::CreateOnly) {
            panic!("atomic write must succeed: {error}");
        }

        let actual = match fs::read(&target) {
            Ok(content) => content,
            Err(error) => panic!("replacement must be readable: {error}"),
        };
        assert_eq!(actual, new);
        let metadata = match fs::metadata(&target) {
            Ok(metadata) => metadata,
            Err(error) => panic!("replacement metadata must be readable: {error}"),
        };
        assert_eq!(metadata.mode() & 0o777, 0o600);
        let entries = match fs::read_dir(directory.path()) {
            Ok(entries) => entries.count(),
            Err(error) => panic!("test directory must be readable: {error}"),
        };
        assert_eq!(entries, 1, "temporary file must not remain after rename");
    }

    #[test]
    fn existing_file_remains_one_complete_old_version() {
        let directory = tempdir();
        let target = directory.path().join("record.json");
        let old = vec![b'o'; 32 * 1024];
        let new = vec![b'n'; 64 * 1024];
        if let Err(error) = fs::write(&target, &old) {
            panic!("old fixture must be written: {error}");
        }

        let result = atomic_write(&target, &new, 0o600, WritePolicy::CreateOnly);

        assert!(matches!(result, Err(AtomicWriteError::Conflict)));
        let actual = match fs::read(&target) {
            Ok(content) => content,
            Err(error) => panic!("old version must remain readable: {error}"),
        };
        assert!(actual == old || actual == new);
        assert_eq!(actual, old);
        let entries = match fs::read_dir(directory.path()) {
            Ok(entries) => entries.count(),
            Err(error) => panic!("test directory must be readable: {error}"),
        };
        assert_eq!(entries, 1, "failed temporary file must be cleaned up");
    }

    #[test]
    fn non_conflict_failures_are_not_reported_as_conflicts() {
        let directory = tempdir();
        let target = directory.path().join("missing").join("record.json");

        let result = atomic_write(&target, b"content", 0o600, WritePolicy::CreateOnly);

        assert!(matches!(result, Err(AtomicWriteError::Failed)));
    }

    #[test]
    fn replace_policy_installs_one_complete_new_version() {
        let directory = tempdir();
        let target = directory.path().join("record.json");
        let old = vec![b'o'; 64 * 1024];
        let new = vec![b'n'; 32 * 1024];
        if let Err(error) = fs::write(&target, &old) {
            panic!("old fixture must be written: {error}");
        }

        if let Err(error) = atomic_write(&target, &new, 0o640, WritePolicy::Replace) {
            panic!("replacement must succeed: {error}");
        }

        let actual = match fs::read(&target) {
            Ok(content) => content,
            Err(error) => panic!("replacement must be readable: {error}"),
        };
        assert_eq!(actual, new);
        let metadata = match fs::metadata(&target) {
            Ok(metadata) => metadata,
            Err(error) => panic!("replacement metadata must be readable: {error}"),
        };
        assert_eq!(metadata.mode() & 0o777, 0o640);
        let entries = match fs::read_dir(directory.path()) {
            Ok(entries) => entries.count(),
            Err(error) => panic!("test directory must be readable: {error}"),
        };
        assert_eq!(
            entries, 1,
            "temporary file must not remain after replacement"
        );
    }

    #[test]
    fn durable_remove_is_idempotent() {
        let directory = tempdir();
        let target = directory.path().join("record.json");
        if let Err(error) = fs::write(&target, b"content") {
            panic!("fixture must be written: {error}");
        }

        if let Err(error) = durable_remove(&target) {
            panic!("existing file must be removed durably: {error}");
        }
        assert!(!target.exists());
        if let Err(error) = durable_remove(&target) {
            panic!("absent file removal must be idempotent: {error}");
        }
    }
}
