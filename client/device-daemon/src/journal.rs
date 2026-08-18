use std::{
    fs::{self, DirBuilder, File},
    io::ErrorKind,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
};

use natsume_device_protocol::is_canonical_command_id;
use snafu::Snafu;

use crate::atomic_write::{AtomicWriteError, WritePolicy, atomic_write};

const JOURNAL_DIRECTORY_MODE: u32 = 0o750;
const JOURNAL_FRAME_MODE: u32 = 0o600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JournalOutcome {
    Recorded,
    AlreadyRecorded,
    Conflict,
}

#[derive(Debug, Snafu)]
pub(crate) enum JournalError {
    #[snafu(display("journal command ID is not a canonical lowercase UUIDv7"))]
    InvalidCommandId,

    #[snafu(display("device command journal is unavailable"))]
    Unavailable,
}

#[derive(Clone)]
pub(crate) struct Journal {
    directory: PathBuf,
}

impl Journal {
    /// Opens an existing journal directory or creates it below an existing state directory.
    ///
    /// # Errors
    ///
    /// Returns a redacted error if the journal path cannot be made into a durable directory.
    pub(crate) fn open(directory: PathBuf) -> Result<Self, JournalError> {
        ensure_directory(&directory)?;
        Ok(Self { directory })
    }

    /// Durably records the exact received frame once and classifies subsequent deliveries.
    ///
    /// The remote command ID is validated before it is converted into a filesystem path.
    ///
    /// # Errors
    ///
    /// Returns a redacted error for a non-canonical command ID or any journal I/O failure.
    pub(crate) fn record(
        &self,
        command_id: &str,
        frame_bytes: &[u8],
    ) -> Result<JournalOutcome, JournalError> {
        validate_command_id(command_id)?;
        let target = self.directory.join(format!("{command_id}.frame"));

        if let Some(outcome) = compare_existing(&target, frame_bytes)? {
            return Ok(outcome);
        }

        match atomic_write(
            &target,
            frame_bytes,
            JOURNAL_FRAME_MODE,
            WritePolicy::CreateOnly,
        ) {
            Ok(()) => Ok(JournalOutcome::Recorded),
            // A concurrent delivery may win the create-only rename. Re-read the winner and
            // classify its exact bytes instead of treating a benign duplicate as an I/O loss.
            Err(AtomicWriteError::Rename) => {
                compare_existing(&target, frame_bytes)?.ok_or(JournalError::Unavailable)
            }
            Err(
                AtomicWriteError::Parent
                | AtomicWriteError::Create
                | AtomicWriteError::Mode
                | AtomicWriteError::Write
                | AtomicWriteError::SyncFile
                | AtomicWriteError::SyncDirectory,
            ) => Err(JournalError::Unavailable),
        }
    }
}

fn validate_command_id(command_id: &str) -> Result<(), JournalError> {
    if !is_canonical_command_id(command_id) {
        return Err(JournalError::InvalidCommandId);
    }
    Ok(())
}

fn compare_existing(
    target: &Path,
    frame_bytes: &[u8],
) -> Result<Option<JournalOutcome>, JournalError> {
    match fs::read(target) {
        Ok(existing) if existing == frame_bytes => Ok(Some(JournalOutcome::AlreadyRecorded)),
        Ok(_) => Ok(Some(JournalOutcome::Conflict)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(_) => Err(JournalError::Unavailable),
    }
}

fn ensure_directory(directory: &Path) -> Result<(), JournalError> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.is_dir() => return Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Ok(_) | Err(_) => return Err(JournalError::Unavailable),
    }

    match DirBuilder::new()
        .mode(JOURNAL_DIRECTORY_MODE)
        .create(directory)
    {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            return fs::symlink_metadata(directory)
                .ok()
                .filter(fs::Metadata::is_dir)
                .map(|_| ())
                .ok_or(JournalError::Unavailable);
        }
        Err(_) => return Err(JournalError::Unavailable),
    }
    let parent = directory.parent().ok_or(JournalError::Unavailable)?;
    File::open(parent)
        .and_then(|parent| parent.sync_all())
        .map_err(|_| JournalError::Unavailable)
}

#[cfg(test)]
mod tests;
