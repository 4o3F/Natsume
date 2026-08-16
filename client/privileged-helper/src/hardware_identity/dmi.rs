use std::path::Path;

use natsume_machine_identity::ReadOutcome;
use zeroize::Zeroize as _;

use super::{
    DMI_BOARD_SERIAL, DMI_DIRECTORY, DMI_SYSTEM_UUID, SourceStatus,
    smbios::{SmbiosReadings, smbios_readings},
    source::{interface_status, outcome_from_status, read_value, rooted},
};

pub(super) fn primary_dmi_readings(filesystem_root: &Path) -> [ReadOutcome; 2] {
    match interface_status(&rooted(filesystem_root, DMI_DIRECTORY)) {
        Ok(()) => [
            read_value(&rooted(filesystem_root, DMI_SYSTEM_UUID)),
            read_value(&rooted(filesystem_root, DMI_BOARD_SERIAL)),
        ],
        Err(status) => [outcome_from_status(status), outcome_from_status(status)],
    }
}

pub(super) fn comparison_characters(value: &str) -> impl Iterator<Item = char> + '_ {
    value
        .trim_matches(|character: char| character.is_ascii_whitespace())
        .chars()
        .filter(|character| !matches!(character, '-' | '_' | ':' | ' '))
        .flat_map(char::to_lowercase)
}

pub(super) fn values_are_equivalent(left: &str, right: &str) -> bool {
    comparison_characters(left).eq(comparison_characters(right))
}

pub(super) fn merge_dmi(primary: ReadOutcome, fallback: Option<String>) -> ReadOutcome {
    match primary {
        ReadOutcome::Value(mut primary_value) => match fallback {
            Some(mut fallback_value) if !values_are_equivalent(&primary_value, &fallback_value) => {
                // The frozen ReadOutcome API has no external-conflict variant. Per WP3, a
                // detected sysfs/SMBIOS disagreement is conservatively made unavailable.
                primary_value.zeroize();
                fallback_value.zeroize();
                ReadOutcome::Unavailable
            }
            Some(mut fallback_value) => {
                fallback_value.zeroize();
                ReadOutcome::Value(primary_value)
            }
            None => ReadOutcome::Value(primary_value),
        },
        ReadOutcome::Unavailable | ReadOutcome::Unsupported => {
            fallback.map_or(ReadOutcome::Unavailable, ReadOutcome::Value)
        }
        ReadOutcome::PermissionDenied => {
            if let Some(mut fallback_value) = fallback {
                fallback_value.zeroize();
            }
            ReadOutcome::PermissionDenied
        }
    }
}

pub(super) fn apply_smbios_failure(
    primary: ReadOutcome,
    smbios_status: SourceStatus,
) -> ReadOutcome {
    match primary {
        ReadOutcome::Value(value) => ReadOutcome::Value(value),
        ReadOutcome::PermissionDenied => ReadOutcome::PermissionDenied,
        ReadOutcome::Unavailable => match smbios_status {
            SourceStatus::PermissionDenied => ReadOutcome::PermissionDenied,
            SourceStatus::Unavailable | SourceStatus::Unsupported => ReadOutcome::Unavailable,
        },
        ReadOutcome::Unsupported => match smbios_status {
            SourceStatus::PermissionDenied => ReadOutcome::PermissionDenied,
            SourceStatus::Unavailable => ReadOutcome::Unavailable,
            SourceStatus::Unsupported => ReadOutcome::Unsupported,
        },
    }
}

pub(super) fn collect_dmi(filesystem_root: &Path) -> [ReadOutcome; 2] {
    let [system_uuid, board_serial] = primary_dmi_readings(filesystem_root);
    match smbios_readings(filesystem_root) {
        SmbiosReadings::Values {
            system_uuid: smbios_system_uuid,
            board_serial: smbios_board_serial,
        } => [
            merge_dmi(system_uuid, smbios_system_uuid),
            merge_dmi(board_serial, smbios_board_serial),
        ],
        SmbiosReadings::Status(status) => [
            apply_smbios_failure(system_uuid, status),
            apply_smbios_failure(board_serial, status),
        ],
    }
}
