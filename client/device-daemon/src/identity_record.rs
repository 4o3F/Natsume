use std::{fs, io::ErrorKind, path::Path};

use natsume_machine_identity::IdentityRecordState;
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    atomic_write::{AtomicWriteError, WritePolicy, atomic_write},
    canonical_uuid,
};

const IDENTITY_RECORD_NAME: &str = "identity.json";
const IDENTITY_RECORD_MODE: u32 = 0o600;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct IdentityRecordDocument {
    fleet_namespace_uuid: String,
    machine_hardware_id: String,
}

#[derive(Debug, Snafu)]
pub(super) enum IdentityRecordWriteError {
    #[snafu(display("identity record already contains a different identity"))]
    ExistingIdentity,

    #[snafu(display("a corrupt identity record may not be overwritten"))]
    ExistingCorrupt,

    #[snafu(display("identity record could not be serialized"))]
    Serialize,

    #[snafu(display("identity record atomic persistence failed"))]
    Atomic { source: AtomicWriteError },
}

fn record_path(identity_directory: &Path) -> std::path::PathBuf {
    identity_directory.join(IDENTITY_RECORD_NAME)
}

fn decode(bytes: &[u8]) -> Option<(Uuid, Uuid)> {
    let document = serde_json::from_slice::<IdentityRecordDocument>(bytes).ok()?;
    Some((
        canonical_uuid(&document.fleet_namespace_uuid)?,
        canonical_uuid(&document.machine_hardware_id)?,
    ))
}

pub(super) fn read(identity_directory: &Path) -> IdentityRecordState {
    match fs::read(record_path(identity_directory)) {
        Ok(bytes) => match decode(&bytes) {
            Some((fleet_namespace_uuid, machine_hardware_id)) => IdentityRecordState::Valid {
                fleet_namespace_uuid,
                machine_hardware_id,
            },
            None => IdentityRecordState::Corrupt,
        },
        Err(error) if error.kind() == ErrorKind::NotFound => IdentityRecordState::Absent,
        Err(_) => IdentityRecordState::Corrupt,
    }
}

pub(super) fn write_first_start(
    identity_directory: &Path,
    fleet_namespace_uuid: Uuid,
    machine_hardware_id: Uuid,
) -> Result<(), IdentityRecordWriteError> {
    match read(identity_directory) {
        IdentityRecordState::Absent => {}
        IdentityRecordState::Corrupt => return Err(IdentityRecordWriteError::ExistingCorrupt),
        IdentityRecordState::Valid {
            fleet_namespace_uuid: stored_namespace,
            machine_hardware_id: stored_machine_hardware_id,
        } if stored_namespace == fleet_namespace_uuid
            && stored_machine_hardware_id == machine_hardware_id =>
        {
            return Ok(());
        }
        IdentityRecordState::Valid { .. } => {
            return Err(IdentityRecordWriteError::ExistingIdentity);
        }
    }

    let document = IdentityRecordDocument {
        fleet_namespace_uuid: fleet_namespace_uuid.to_string(),
        machine_hardware_id: machine_hardware_id.to_string(),
    };
    let bytes = serde_json::to_vec(&document).map_err(|_| IdentityRecordWriteError::Serialize)?;
    atomic_write(
        &record_path(identity_directory),
        &bytes,
        IDENTITY_RECORD_MODE,
        WritePolicy::CreateOnly,
    )
    .map_err(|source| IdentityRecordWriteError::Atomic { source })
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::MetadataExt as _};

    use tempfile::TempDir;

    use super::*;

    const NAMESPACE: Uuid = Uuid::from_u128(0x1234_5678_1234_5678_9234_5678_1234_5678);
    const MACHINE_ID: Uuid = Uuid::from_u128(0xa9aa_9d04_3ece_5567_8260_9109_30ff_5e03);

    fn tempdir() -> TempDir {
        match TempDir::new() {
            Ok(directory) => directory,
            Err(error) => panic!("test directory must be created: {error}"),
        }
    }

    fn write_raw(directory: &Path, bytes: &[u8]) {
        if let Err(error) = fs::write(record_path(directory), bytes) {
            panic!("identity fixture must be written: {error}");
        }
    }

    #[test]
    fn absent_record_is_classified() {
        let directory = tempdir();
        assert_eq!(read(directory.path()), IdentityRecordState::Absent);
    }

    #[test]
    fn valid_record_is_classified() {
        let directory = tempdir();
        write_raw(
            directory.path(),
            br#"{"fleet_namespace_uuid":"12345678-1234-5678-9234-567812345678","machine_hardware_id":"a9aa9d04-3ece-5567-8260-910930ff5e03"}"#,
        );

        assert_eq!(
            read(directory.path()),
            IdentityRecordState::Valid {
                fleet_namespace_uuid: NAMESPACE,
                machine_hardware_id: MACHINE_ID,
            }
        );
    }

    #[test]
    fn malformed_and_noncanonical_records_are_corrupt() {
        let cases: &[&[u8]] = &[
            b"{}",
            br#"{"fleet_namespace_uuid":"12345678-1234-5678-9234-567812345678","machine_hardware_id":"a9aa9d04-3ece-5567-8260-910930ff5e03","extra":true}"#,
            br#"{"fleet_namespace_uuid":"12345678-1234-5678-9234-567812345678","machine_hardware_id":"A9AA9D04-3ECE-5567-8260-910930FF5E03"}"#,
            br#"{"fleet_namespace_uuid":"12345678123456789234567812345678","machine_hardware_id":"a9aa9d04-3ece-5567-8260-910930ff5e03"}"#,
            br#"{"fleet_namespace_uuid":"12345678-1234-5678-9234-567812345678"}"#,
            br#"{"fleet_namespace_uuid":"12345678-1234-5678-9234-567812345678","machine_hardware_id":"a9aa9d04""#,
            b"not json",
            b"\xff\xfe\x00",
        ];

        for bytes in cases {
            let directory = tempdir();
            write_raw(directory.path(), bytes);
            assert_eq!(read(directory.path()), IdentityRecordState::Corrupt);
        }
    }

    #[test]
    fn io_failure_on_a_present_record_is_corrupt() {
        let directory = tempdir();
        if let Err(error) = fs::create_dir(record_path(directory.path())) {
            panic!("directory-shaped record fixture must be created: {error}");
        }
        assert_eq!(read(directory.path()), IdentityRecordState::Corrupt);
    }

    #[test]
    fn first_start_write_is_exact_and_owner_only() {
        let directory = tempdir();
        if let Err(error) = write_first_start(directory.path(), NAMESPACE, MACHINE_ID) {
            panic!("identity record must be written: {error}");
        }

        let path = record_path(directory.path());
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => panic!("identity record must be readable: {error}"),
        };
        assert_eq!(
            content,
            r#"{"fleet_namespace_uuid":"12345678-1234-5678-9234-567812345678","machine_hardware_id":"a9aa9d04-3ece-5567-8260-910930ff5e03"}"#
        );
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => panic!("identity metadata must be readable: {error}"),
        };
        assert_eq!(metadata.mode() & 0o777, IDENTITY_RECORD_MODE);
    }

    #[test]
    fn existing_different_valid_record_is_never_overwritten() {
        let directory = tempdir();
        if let Err(error) = write_first_start(directory.path(), NAMESPACE, MACHINE_ID) {
            panic!("initial identity record must be written: {error}");
        }
        let result = write_first_start(directory.path(), NAMESPACE, Uuid::from_u128(1));

        assert!(matches!(
            result,
            Err(IdentityRecordWriteError::ExistingIdentity)
        ));
        assert_eq!(
            read(directory.path()),
            IdentityRecordState::Valid {
                fleet_namespace_uuid: NAMESPACE,
                machine_hardware_id: MACHINE_ID,
            }
        );
    }
}
