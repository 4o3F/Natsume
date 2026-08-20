use std::{
    fs::{self, File},
    io::Write as _,
    os::unix::fs::PermissionsExt as _,
    path::Path,
};

use snafu::Snafu;
use tempfile::Builder;

pub(crate) const ATOMIC_TEMP_PREFIX: &str = ".natsume-tmp";

#[derive(Clone, Copy)]
pub(super) enum WritePolicy {
    CreateOnly,
    #[allow(dead_code)]
    Replace,
}

#[derive(Debug, Snafu)]
pub(super) enum AtomicWriteError {
    #[snafu(display("atomic file target has no parent directory"))]
    Parent,

    #[snafu(display("atomic file temporary file could not be created"))]
    Create,

    #[snafu(display("atomic file mode could not be set"))]
    Mode,

    #[snafu(display("atomic file content could not be written"))]
    Write,

    #[snafu(display("atomic file content could not be synchronized"))]
    SyncFile,

    #[snafu(display("atomic file could not be renamed into place"))]
    Rename,

    #[snafu(display("atomic file directory could not be synchronized"))]
    SyncDirectory,
}

/// Writes one complete file through a same-directory temporary file, fsync, rename, and directory
/// fsync. Mode is the only policy input; ownership remains that of the calling service user.
pub(super) fn atomic_write(
    target: &Path,
    content: &[u8],
    mode: u32,
    policy: WritePolicy,
) -> Result<(), AtomicWriteError> {
    let parent = target.parent().ok_or(AtomicWriteError::Parent)?;
    let mut temporary = Builder::new()
        .prefix(ATOMIC_TEMP_PREFIX)
        .tempfile_in(parent)
        .map_err(|_| AtomicWriteError::Create)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|_| AtomicWriteError::Mode)?;
    temporary
        .write_all(content)
        .map_err(|_| AtomicWriteError::Write)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| AtomicWriteError::SyncFile)?;
    match policy {
        WritePolicy::CreateOnly => rustix::fs::renameat_with(
            rustix::fs::CWD,
            temporary.path(),
            rustix::fs::CWD,
            target,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(|_| AtomicWriteError::Rename)?,
        WritePolicy::Replace => {
            fs::rename(temporary.path(), target).map_err(|_| AtomicWriteError::Rename)?;
        }
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| AtomicWriteError::SyncDirectory)
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

        assert!(matches!(result, Err(AtomicWriteError::Rename)));
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
}
