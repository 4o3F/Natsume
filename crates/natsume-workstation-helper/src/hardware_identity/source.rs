use std::{
    fs,
    io::{self, ErrorKind},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

use super::{SourceStatus, policy::ReadOutcome};

pub(super) fn rooted(filesystem_root: &Path, absolute_path: &str) -> PathBuf {
    let relative = Path::new(absolute_path)
        .strip_prefix("/")
        .unwrap_or_else(|_| Path::new(absolute_path));
    filesystem_root.join(relative)
}

pub(super) fn interface_status(path: &Path) -> Result<(), SourceStatus> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(SourceStatus::Unsupported),
        Err(error) => Err(match error.kind() {
            ErrorKind::PermissionDenied => SourceStatus::PermissionDenied,
            ErrorKind::NotFound | ErrorKind::Unsupported => SourceStatus::Unsupported,
            _ => SourceStatus::Unavailable,
        }),
    }
}

pub(super) fn io_status(error: &io::Error) -> SourceStatus {
    match error.kind() {
        ErrorKind::PermissionDenied => SourceStatus::PermissionDenied,
        ErrorKind::Unsupported => SourceStatus::Unsupported,
        _ => SourceStatus::Unavailable,
    }
}

pub(super) fn read_bytes(path: &Path) -> Result<Vec<u8>, SourceStatus> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.permissions().mode() & 0o444 == 0 => {
            return Err(SourceStatus::PermissionDenied);
        }
        Ok(_) => {}
        Err(error) => return Err(io_status(&error)),
    }
    fs::read(path).map_err(|error| io_status(&error))
}

pub(super) fn bytes_as_value(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(value) => value,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    }
}

pub(super) fn read_value(path: &Path) -> ReadOutcome {
    match read_bytes(path) {
        Ok(bytes) => ReadOutcome::Value(bytes_as_value(bytes)),
        Err(status) => outcome_from_status(status),
    }
}

pub(super) fn outcome_from_status(status: SourceStatus) -> ReadOutcome {
    match status {
        SourceStatus::Unavailable => ReadOutcome::Unavailable,
        SourceStatus::PermissionDenied => ReadOutcome::PermissionDenied,
        SourceStatus::Unsupported => ReadOutcome::Unsupported,
    }
}
