use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use snafu::Snafu;
use zeroize::Zeroize;

const MASTER_KEY_LENGTH: usize = 32;
const PRIVATE_FILE_FORBIDDEN_BITS: u32 = 0o177;
const PRIVATE_DIRECTORY_FORBIDDEN_BITS: u32 = 0o077;

/// Ensures that the Server vault master key exists and satisfies its storage
/// policy.
///
/// # Errors
///
/// Returns a redacted [`VaultError`] when directory policy, key policy,
/// entropy acquisition, or atomic persistence fails.
pub(crate) fn ensure_master_key(master_key_path: &Path) -> Result<(), VaultError> {
    validate_private_directory(master_key_path)?;
    let master_key = match OpenOptions::new().read(true).open(master_key_path) {
        Ok(file) => read_existing_key(file)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_master_key(master_key_path)?
        }
        Err(_) => return Err(VaultError::InvalidExistingKey),
    };
    drop(master_key);
    Ok(())
}

/// Requires an existing Server vault master key and validates its storage
/// policy without creating any filesystem artifact.
///
/// # Errors
///
/// Returns a redacted [`VaultError`] when the key is absent or invalid.
pub(crate) fn require_master_key(master_key_path: &Path) -> Result<(), VaultError> {
    validate_private_directory(master_key_path)?;
    let file = OpenOptions::new()
        .read(true)
        .open(master_key_path)
        .map_err(|_| VaultError::InvalidExistingKey)?;
    drop(read_existing_key(file)?);
    Ok(())
}

struct VaultMasterKey {
    bytes: [u8; MASTER_KEY_LENGTH],
}

impl VaultMasterKey {
    fn generate() -> Result<Self, VaultError> {
        let mut key = Self {
            bytes: [0; MASTER_KEY_LENGTH],
        };
        getrandom::fill(&mut key.bytes).map_err(|_| VaultError::EntropyUnavailable)?;
        Ok(key)
    }

    const fn empty() -> Self {
        Self {
            bytes: [0; MASTER_KEY_LENGTH],
        }
    }

    const fn expose(&self) -> &[u8; MASTER_KEY_LENGTH] {
        &self.bytes
    }

    fn expose_mut(&mut self) -> &mut [u8; MASTER_KEY_LENGTH] {
        &mut self.bytes
    }
}

impl Drop for VaultMasterKey {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

fn validate_private_directory(master_key_path: &Path) -> Result<(), VaultError> {
    let parent = master_key_path
        .parent()
        .ok_or(VaultError::InvalidExistingKey)?;
    let metadata = fs::metadata(parent).map_err(|_| VaultError::InvalidExistingKey)?;
    if !metadata.is_dir() {
        return Err(VaultError::InvalidExistingKey);
    }
    if metadata.permissions().mode() & PRIVATE_DIRECTORY_FORBIDDEN_BITS != 0 {
        return Err(VaultError::InvalidExistingKey);
    }
    Ok(())
}

fn read_existing_key(mut file: File) -> Result<VaultMasterKey, VaultError> {
    let metadata = file
        .metadata()
        .map_err(|_| VaultError::InvalidExistingKey)?;
    if !metadata.is_file() {
        return Err(VaultError::InvalidExistingKey);
    }
    if metadata.permissions().mode() & PRIVATE_FILE_FORBIDDEN_BITS != 0 {
        return Err(VaultError::InvalidExistingKey);
    }

    let mut key = VaultMasterKey::empty();
    file.read_exact(key.expose_mut())
        .map_err(|_| VaultError::InvalidExistingKey)?;
    let mut extra = [0_u8; 1];
    match file.read(&mut extra) {
        Ok(0) => Ok(key),
        Ok(_) | Err(_) => Err(VaultError::InvalidExistingKey),
    }
}

fn create_master_key(master_key_path: &Path) -> Result<VaultMasterKey, VaultError> {
    let temporary_path = temporary_key_path(master_key_path);
    let mut temporary_file = TemporaryKeyFile::claim(temporary_path)?;
    match OpenOptions::new().read(true).open(master_key_path) {
        Ok(file) => return read_existing_key(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(VaultError::InvalidExistingKey),
    }

    let key = VaultMasterKey::generate()?;
    temporary_file.write_and_sync(key.expose())?;
    temporary_file.install(master_key_path)?;
    sync_parent_directory(master_key_path)?;
    Ok(key)
}

struct TemporaryKeyFile {
    path: PathBuf,
    file: Option<File>,
    installed: bool,
}

impl TemporaryKeyFile {
    fn claim(path: PathBuf) -> Result<Self, VaultError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| VaultError::PersistenceFailed)?;
        Ok(Self {
            path,
            file: Some(file),
            installed: false,
        })
    }

    fn write_and_sync(&mut self, key: &[u8; MASTER_KEY_LENGTH]) -> Result<(), VaultError> {
        let file = self.file.as_mut().ok_or(VaultError::PersistenceFailed)?;
        file.write_all(key)
            .map_err(|_| VaultError::PersistenceFailed)?;
        file.sync_all().map_err(|_| VaultError::PersistenceFailed)
    }

    fn install(&mut self, master_key_path: &Path) -> Result<(), VaultError> {
        drop(self.file.take());
        fs::rename(&self.path, master_key_path).map_err(|_| VaultError::PersistenceFailed)?;
        self.installed = true;
        Ok(())
    }
}

impl Drop for TemporaryKeyFile {
    fn drop(&mut self) {
        if !self.installed && fs::remove_file(&self.path).is_err() {
            tracing::error!("vault temporary key file cleanup failed");
        }
    }
}

fn temporary_key_path(master_key_path: &Path) -> PathBuf {
    master_key_path.with_extension("tmp")
}

fn sync_parent_directory(master_key_path: &Path) -> Result<(), VaultError> {
    let parent = master_key_path
        .parent()
        .ok_or(VaultError::PersistenceFailed)?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| VaultError::PersistenceFailed)
}

/// Redacted vault master-key lifecycle failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum VaultError {
    #[snafu(display("the existing vault master key is invalid"))]
    InvalidExistingKey,
    #[snafu(display("operating-system entropy is unavailable"))]
    EntropyUnavailable,
    #[snafu(display("the vault master key could not be persisted"))]
    PersistenceFailed,
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
        path::{Path, PathBuf},
        sync::{Arc, Barrier},
        thread,
    };

    use snafu::Snafu;
    use uuid::Uuid;
    use zeroize::Zeroizing;

    use crate::{
        config::LogLevel,
        logging::tests::{CapturedLogs, SubscriberTestGuard},
    };

    use super::{
        MASTER_KEY_LENGTH, TemporaryKeyFile, VaultError, create_master_key, ensure_master_key,
        require_master_key, temporary_key_path,
    };

    #[test]
    fn first_start_creates_private_key_file() -> Result<(), TestFailure> {
        let directory = TestDirectory::new(0o700)?;
        let key_path = directory.path.join("master.key");

        ensure_master_key(&key_path).map_err(|_| TestFailure::UnexpectedVaultFailure)?;

        let metadata = fs::metadata(&key_path).map_err(|_| TestFailure::FixtureIoFailed)?;
        if metadata.len() != MASTER_KEY_LENGTH as u64 {
            return Err(TestFailure::UnexpectedKeyLength);
        }
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return Err(TestFailure::UnexpectedKeyMode);
        }
        Ok(())
    }

    #[test]
    fn temporary_key_cleanup_failure_is_reported_without_context() -> Result<(), TestFailure> {
        let _subscriber_guard = SubscriberTestGuard::acquire();
        let directory = TestDirectory::new(0o700)?;
        let temporary_path = directory.path.join("cleanup-failure-canary.tmp");
        let temporary = TemporaryKeyFile::claim(temporary_path.clone())
            .map_err(|_| TestFailure::UnexpectedVaultFailure)?;
        fs::remove_file(&temporary_path).map_err(|_| TestFailure::FixtureIoFailed)?;
        fs::create_dir(&temporary_path).map_err(|_| TestFailure::FixtureIoFailed)?;

        let captured = CapturedLogs::default();
        let subscriber = captured.subscriber(LogLevel::Error);
        tracing::subscriber::with_default(subscriber, || drop(temporary));
        let output = captured
            .text()
            .map_err(|()| TestFailure::LogCaptureFailed)?;
        if output
            .matches("vault temporary key file cleanup failed")
            .count()
            != 1
            || output.contains(temporary_path.to_string_lossy().as_ref())
        {
            return Err(TestFailure::TemporaryCleanupFailureWasNotReported);
        }
        Ok(())
    }

    #[test]
    fn second_open_reads_without_rewriting() -> Result<(), TestFailure> {
        let directory = TestDirectory::new(0o700)?;
        let key_path = directory.path.join("master.key");
        ensure_master_key(&key_path).map_err(|_| TestFailure::UnexpectedVaultFailure)?;
        let content_before =
            Zeroizing::new(fs::read(&key_path).map_err(|_| TestFailure::FixtureIoFailed)?);
        let modified_before = fs::metadata(&key_path)
            .and_then(|metadata| metadata.modified())
            .map_err(|_| TestFailure::FixtureIoFailed)?;

        ensure_master_key(&key_path).map_err(|_| TestFailure::UnexpectedVaultFailure)?;

        let content_after =
            Zeroizing::new(fs::read(&key_path).map_err(|_| TestFailure::FixtureIoFailed)?);
        let modified_after = fs::metadata(&key_path)
            .and_then(|metadata| metadata.modified())
            .map_err(|_| TestFailure::FixtureIoFailed)?;
        if content_before.as_slice() != content_after.as_slice() {
            return Err(TestFailure::ExistingKeyWasRewritten);
        }
        if modified_before != modified_after {
            return Err(TestFailure::ExistingKeyWasRewritten);
        }
        Ok(())
    }

    #[test]
    fn require_reads_without_rewriting() -> Result<(), TestFailure> {
        let directory = TestDirectory::new(0o700)?;
        let key_path = directory.path.join("master.key");
        ensure_master_key(&key_path).map_err(|_| TestFailure::UnexpectedVaultFailure)?;
        let content_before =
            Zeroizing::new(fs::read(&key_path).map_err(|_| TestFailure::FixtureIoFailed)?);
        let modified_before = fs::metadata(&key_path)
            .and_then(|metadata| metadata.modified())
            .map_err(|_| TestFailure::FixtureIoFailed)?;

        require_master_key(&key_path).map_err(|_| TestFailure::UnexpectedVaultFailure)?;

        let content_after =
            Zeroizing::new(fs::read(&key_path).map_err(|_| TestFailure::FixtureIoFailed)?);
        let modified_after = fs::metadata(&key_path)
            .and_then(|metadata| metadata.modified())
            .map_err(|_| TestFailure::FixtureIoFailed)?;
        if content_before.as_slice() != content_after.as_slice()
            || modified_before != modified_after
        {
            return Err(TestFailure::ExistingKeyWasRewritten);
        }
        Ok(())
    }

    #[test]
    fn require_missing_key_creates_no_artifacts() -> Result<(), TestFailure> {
        let directory = TestDirectory::new(0o700)?;
        let key_path = directory.path.join("master.key");
        let temporary_path = temporary_key_path(&key_path);

        let Err(error) = require_master_key(&key_path) else {
            return Err(TestFailure::ExpectedVaultFailure);
        };
        if error != VaultError::InvalidExistingKey {
            return Err(TestFailure::UnexpectedVaultFailure);
        }
        if key_path.exists() || temporary_path.exists() {
            return Err(TestFailure::UnexpectedKeyArtifact);
        }
        Ok(())
    }

    #[test]
    fn wrong_length_file_fails_closed() -> Result<(), TestFailure> {
        let directory = TestDirectory::new(0o700)?;
        let key_path = directory.path.join("master.key");
        install_key_fixture(&key_path, &[0_u8; MASTER_KEY_LENGTH - 1], 0o600)?;
        assert_vault_error(&key_path, VaultError::InvalidExistingKey)
    }

    #[test]
    fn world_readable_file_fails_closed() -> Result<(), TestFailure> {
        let directory = TestDirectory::new(0o700)?;
        let key_path = directory.path.join("master.key");
        install_key_fixture(&key_path, &[0_u8; MASTER_KEY_LENGTH], 0o644)?;
        assert_vault_error(&key_path, VaultError::InvalidExistingKey)
    }

    #[test]
    fn wide_directory_mode_fails_closed() -> Result<(), TestFailure> {
        let directory = TestDirectory::new(0o755)?;
        let key_path = directory.path.join("master.key");
        assert_vault_error(&key_path, VaultError::InvalidExistingKey)
    }

    #[test]
    fn stale_temporary_files_fail_closed_without_promotion() -> Result<(), TestFailure> {
        for mode in [0o600, 0o644] {
            let directory = TestDirectory::new(0o700)?;
            let key_path = directory.path.join("master.key");
            let temporary_path = temporary_key_path(&key_path);
            install_key_fixture(&temporary_path, b"stale temporary contents", mode)?;

            assert_vault_error(&key_path, VaultError::PersistenceFailed)?;
            if key_path.exists() || !temporary_path.exists() {
                return Err(TestFailure::StaleTemporaryFileWasPromoted);
            }
        }
        Ok(())
    }

    #[test]
    fn concurrent_first_start_preserves_one_key() -> Result<(), TestFailure> {
        let directory = TestDirectory::new(0o700)?;
        let key_path = directory.path.join("master.key");
        let barrier = Arc::new(Barrier::new(3));

        let first_path = key_path.clone();
        let first_barrier = Arc::clone(&barrier);
        let first = thread::spawn(move || {
            first_barrier.wait();
            ensure_master_key(&first_path)
        });
        let second_path = key_path.clone();
        let second_barrier = Arc::clone(&barrier);
        let second = thread::spawn(move || {
            second_barrier.wait();
            ensure_master_key(&second_path)
        });
        barrier.wait();

        let first_result = first.join().map_err(|_| TestFailure::ThreadFailed)?;
        let second_result = second.join().map_err(|_| TestFailure::ThreadFailed)?;
        if first_result.is_err() && second_result.is_err() {
            return Err(TestFailure::ConcurrentFirstStartFailed);
        }

        let content_before =
            Zeroizing::new(fs::read(&key_path).map_err(|_| TestFailure::FixtureIoFailed)?);
        let modified_before = fs::metadata(&key_path)
            .and_then(|metadata| metadata.modified())
            .map_err(|_| TestFailure::FixtureIoFailed)?;
        ensure_master_key(&key_path).map_err(|_| TestFailure::UnexpectedVaultFailure)?;
        ensure_master_key(&key_path).map_err(|_| TestFailure::UnexpectedVaultFailure)?;
        let content_after =
            Zeroizing::new(fs::read(&key_path).map_err(|_| TestFailure::FixtureIoFailed)?);
        let modified_after = fs::metadata(&key_path)
            .and_then(|metadata| metadata.modified())
            .map_err(|_| TestFailure::FixtureIoFailed)?;
        if content_before.len() != MASTER_KEY_LENGTH
            || content_before.as_slice() != content_after.as_slice()
            || modified_before != modified_after
        {
            return Err(TestFailure::ExistingKeyWasRewritten);
        }
        Ok(())
    }

    #[test]
    fn creation_recheck_preserves_an_installed_winner() -> Result<(), TestFailure> {
        let directory = TestDirectory::new(0o700)?;
        let key_path = directory.path.join("master.key");
        let installed = [0x5a_u8; MASTER_KEY_LENGTH];
        install_key_fixture(&key_path, &installed, 0o600)?;

        let observed =
            create_master_key(&key_path).map_err(|_| TestFailure::UnexpectedVaultFailure)?;
        let persisted =
            Zeroizing::new(fs::read(&key_path).map_err(|_| TestFailure::FixtureIoFailed)?);
        if observed.expose() != &installed || persisted.as_slice() != installed {
            return Err(TestFailure::ExistingKeyWasRewritten);
        }
        if temporary_key_path(&key_path).exists() {
            return Err(TestFailure::FixtureIoFailed);
        }
        Ok(())
    }

    #[test]
    fn generated_keys_are_nonzero_and_distinct() -> Result<(), TestFailure> {
        let first_directory = TestDirectory::new(0o700)?;
        let first_path = first_directory.path.join("master.key");
        let second_directory = TestDirectory::new(0o700)?;
        let second_path = second_directory.path.join("master.key");
        ensure_master_key(&first_path).map_err(|_| TestFailure::UnexpectedVaultFailure)?;
        ensure_master_key(&second_path).map_err(|_| TestFailure::UnexpectedVaultFailure)?;

        let first =
            Zeroizing::new(fs::read(&first_path).map_err(|_| TestFailure::FixtureIoFailed)?);
        let second =
            Zeroizing::new(fs::read(&second_path).map_err(|_| TestFailure::FixtureIoFailed)?);
        if first.len() != MASTER_KEY_LENGTH
            || second.len() != MASTER_KEY_LENGTH
            || first.iter().all(|byte| *byte == 0)
            || second.iter().all(|byte| *byte == 0)
            || first.as_slice() == second.as_slice()
        {
            return Err(TestFailure::GeneratedKeysWereInvalid);
        }
        Ok(())
    }

    fn assert_vault_error(path: &Path, expected: VaultError) -> Result<(), TestFailure> {
        let Err(error) = ensure_master_key(path) else {
            return Err(TestFailure::ExpectedVaultFailure);
        };
        if error != expected {
            return Err(TestFailure::UnexpectedVaultFailure);
        }
        let display = error.to_string();
        let debug = format!("{error:?}");
        let path_canary = path.to_string_lossy();
        if display.contains(path_canary.as_ref()) || debug.contains(path_canary.as_ref()) {
            return Err(TestFailure::VaultErrorWasNotRedacted);
        }
        Ok(())
    }

    fn install_key_fixture(path: &Path, contents: &[u8], mode: u32) -> Result<(), TestFailure> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)
            .map_err(|_| TestFailure::FixtureIoFailed)?;
        file.write_all(contents)
            .map_err(|_| TestFailure::FixtureIoFailed)?;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|_| TestFailure::FixtureIoFailed)
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new(mode: u32) -> Result<Self, TestFailure> {
            let path =
                std::env::temp_dir().join(format!("natsume-server-vault-test-{}", Uuid::now_v7()));
            fs::create_dir(&path).map_err(|_| TestFailure::FixtureIoFailed)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))
                .map_err(|_| TestFailure::FixtureIoFailed)?;
            Ok(Self { path })
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _cleanup_result = fs::remove_dir_all(&self.path);
        }
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the vault fixture operation failed"))]
        FixtureIoFailed,
        #[snafu(display("captured vault logs could not be read"))]
        LogCaptureFailed,
        #[snafu(display("a vault temporary-key cleanup failure was not reported safely"))]
        TemporaryCleanupFailureWasNotReported,
        #[snafu(display("a vault failure was expected"))]
        ExpectedVaultFailure,
        #[snafu(display("the vault result was unexpected"))]
        UnexpectedVaultFailure,
        #[snafu(display("the generated vault master key length was unexpected"))]
        UnexpectedKeyLength,
        #[snafu(display("the generated vault master key mode was unexpected"))]
        UnexpectedKeyMode,
        #[snafu(display("an existing vault master key was rewritten"))]
        ExistingKeyWasRewritten,
        #[snafu(display("a stale temporary vault key file was promoted"))]
        StaleTemporaryFileWasPromoted,
        #[snafu(display("concurrent vault first-start callers all failed"))]
        ConcurrentFirstStartFailed,
        #[snafu(display("a vault test thread failed"))]
        ThreadFailed,
        #[snafu(display("generated vault master keys failed behavioral checks"))]
        GeneratedKeysWereInvalid,
        #[snafu(display("the read-only vault path created a key artifact"))]
        UnexpectedKeyArtifact,
        #[snafu(display("a vault error exposed rejected context"))]
        VaultErrorWasNotRedacted,
    }
}
