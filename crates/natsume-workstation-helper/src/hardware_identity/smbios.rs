use std::path::Path;

use smbioslib::{
    SMBiosBaseboardInformation, SMBiosData, SMBiosEntryPoint32, SMBiosEntryPoint64,
    SMBiosSystemInformation, SMBiosVersion, SystemUuidData,
};

use super::{
    SMBIOS_DIRECTORY, SMBIOS_ENTRY_POINT, SMBIOS_TABLE, SourceStatus,
    source::{interface_status, read_bytes, rooted},
};

pub(super) enum SmbiosReadings {
    Values {
        system_uuid: Option<String>,
        board_serial: Option<String>,
    },
    Status(SourceStatus),
}

pub(super) fn smbios_version(entry_point: Vec<u8>) -> Result<SMBiosVersion, SourceStatus> {
    if entry_point.starts_with(b"_SM3_") {
        SMBiosEntryPoint64::try_from(entry_point)
            .map(|entry| {
                SMBiosVersion::new(entry.major_version(), entry.minor_version(), entry.docrev())
            })
            .map_err(|_| SourceStatus::Unavailable)
    } else {
        SMBiosEntryPoint32::try_from(entry_point)
            .map(|entry| SMBiosVersion::new(entry.major_version(), entry.minor_version(), 0))
            .map_err(|_| SourceStatus::Unavailable)
    }
}

pub(super) fn smbios_readings(filesystem_root: &Path) -> SmbiosReadings {
    if let Err(status) = interface_status(&rooted(filesystem_root, SMBIOS_DIRECTORY)) {
        return SmbiosReadings::Status(status);
    }

    let entry_point = match read_bytes(&rooted(filesystem_root, SMBIOS_ENTRY_POINT)) {
        Ok(bytes) => bytes,
        Err(status) => return SmbiosReadings::Status(status),
    };
    let version = match smbios_version(entry_point) {
        Ok(version) => version,
        Err(status) => return SmbiosReadings::Status(status),
    };
    let table = match read_bytes(&rooted(filesystem_root, SMBIOS_TABLE)) {
        Ok(bytes) => bytes,
        Err(status) => return SmbiosReadings::Status(status),
    };
    let data = SMBiosData::from_vec_and_version(table, Some(version));

    let system_uuid =
        data.find_map(
            |information: SMBiosSystemInformation<'_>| match information.uuid()? {
                SystemUuidData::Uuid(uuid) => Some(uuid.to_string()),
                SystemUuidData::IdNotPresentButSettable | SystemUuidData::IdNotPresent => None,
            },
        );
    let board_serial = data.find_map(|board: SMBiosBaseboardInformation<'_>| {
        board
            .serial_number()
            .to_utf8_lossy()
            .filter(|value| !value.is_empty())
    });

    SmbiosReadings::Values {
        system_uuid,
        board_serial,
    }
}
