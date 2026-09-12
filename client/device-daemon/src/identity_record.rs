use std::{fs, io::ErrorKind, path::Path};

use serde::{Deserialize, Serialize};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    atomic_write::{WritePolicy, atomic_write},
    canonical_uuid,
};

const IDENTITY_RECORD_NAME: &str = "identity.json";
const IDENTITY_RECORD_MODE: u32 = 0o600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IdentityRecordState {
    Absent,
    Corrupt,
    Valid { machine_hardware_id: Uuid },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct IdentityRecordDocument {
    machine_hardware_id: String,
}

#[derive(Debug, Snafu)]
#[snafu(display("identity record could not be persisted"))]
pub(super) struct IdentityRecordWriteError;

fn record_path(identity_directory: &Path) -> std::path::PathBuf {
    identity_directory.join(IDENTITY_RECORD_NAME)
}

fn decode(bytes: &[u8]) -> Option<Uuid> {
    let document = serde_json::from_slice::<IdentityRecordDocument>(bytes).ok()?;
    canonical_uuid(&document.machine_hardware_id)
}

pub(super) fn read(identity_directory: &Path) -> IdentityRecordState {
    match fs::read(record_path(identity_directory)) {
        Ok(bytes) => match decode(&bytes) {
            Some(machine_hardware_id) => IdentityRecordState::Valid {
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
    machine_hardware_id: Uuid,
) -> Result<(), IdentityRecordWriteError> {
    match read(identity_directory) {
        IdentityRecordState::Absent => {}
        IdentityRecordState::Corrupt => return Err(IdentityRecordWriteError),
        IdentityRecordState::Valid {
            machine_hardware_id: stored_machine_hardware_id,
        } if stored_machine_hardware_id == machine_hardware_id => {
            return Ok(());
        }
        IdentityRecordState::Valid { .. } => {
            return Err(IdentityRecordWriteError);
        }
    }

    let document = IdentityRecordDocument {
        machine_hardware_id: machine_hardware_id.to_string(),
    };
    let bytes = serde_json::to_vec(&document).map_err(|_| IdentityRecordWriteError)?;
    atomic_write(
        &record_path(identity_directory),
        &bytes,
        IDENTITY_RECORD_MODE,
        WritePolicy::CreateOnly,
    )
    .map_err(|_| IdentityRecordWriteError)
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::MetadataExt as _};

    use tempfile::TempDir;

    use super::*;

    const MACHINE_ID: Uuid = Uuid::from_u128(0x0c0f_ef01_1126_5297_a522_92cf_e494_ce48);

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
            br#"{"machine_hardware_id":"0c0fef01-1126-5297-a522-92cfe494ce48"}"#,
        );

        assert_eq!(
            read(directory.path()),
            IdentityRecordState::Valid {
                machine_hardware_id: MACHINE_ID,
            }
        );
    }

    #[test]
    fn malformed_and_noncanonical_records_are_corrupt() {
        let cases: &[&[u8]] = &[
            b"{}",
            br#"{"machine_hardware_id":"0c0fef01-1126-5297-a522-92cfe494ce48","extra":true}"#,
            br#"{"machine_hardware_id":"0C0FEF01-1126-5297-A522-92CFE494CE48"}"#,
            br#"{"machine_hardware_id":"0c0fef0111265297a52292cfe494ce48"}"#,
            br#"{"machine_hardware_id":null}"#,
            br#"{"machine_hardware_id":"a9aa9d04""#,
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
        if let Err(error) = write_first_start(directory.path(), MACHINE_ID) {
            panic!("identity record must be written: {error}");
        }

        let path = record_path(directory.path());
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => panic!("identity record must be readable: {error}"),
        };
        assert_eq!(
            content,
            r#"{"machine_hardware_id":"0c0fef01-1126-5297-a522-92cfe494ce48"}"#
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
        if let Err(error) = write_first_start(directory.path(), MACHINE_ID) {
            panic!("initial identity record must be written: {error}");
        }
        let result = write_first_start(directory.path(), Uuid::from_u128(1));

        assert!(result.is_err());
        assert_eq!(
            read(directory.path()),
            IdentityRecordState::Valid {
                machine_hardware_id: MACHINE_ID,
            }
        );
    }

    #[test]
    fn incompatible_record_is_never_replaced_on_first_start() {
        let directory = tempdir();
        let record = br#"{"machine_hardware_id":"0c0fef01-1126-5297-a522-92cfe494ce48","obsolete_field":true}"#;
        write_raw(directory.path(), record);

        assert!(write_first_start(directory.path(), MACHINE_ID).is_err());
        assert_eq!(read(directory.path()), IdentityRecordState::Corrupt);
        assert_eq!(
            fs::read(record_path(directory.path()))
                .unwrap_or_else(|error| panic!("original record must remain readable: {error}")),
            record
        );
    }
}
