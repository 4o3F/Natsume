//! Linux hardware collection belongs to the privileged helper, not the shared crate.
//!
//! Collection order:
//! 1. DMI system UUID and motherboard serial from `/sys/class/dmi/id`.
//! 2. `smbios-lib` only to fill missing DMI values and cross-check present values.
//! 3. `raw-cpuid` only when a genuine Processor Serial Number leaf is advertised.
//! 4. `procfs::process::MountInfo` plus sysfs and udev data for one root whole disk.
//!
//! No app-local or `/etc/machine-id` fallback is accepted because it can be copied with the disk.
//! Raw serials are normalized and hashed in this process, then zeroized or dropped. No shell
//! commands and no text parsing of dmidecode, lsblk, udevadm, or findmnt output are allowed.

use std::{
    collections::{BTreeSet, HashSet},
    fs,
    io::{self, ErrorKind},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

use natsume_local_control_api::{HardwareCandidate, SanitizedHardwareClaim};
use natsume_machine_identity::{
    ANCHOR_ORDER, CollectionCompleteness, EvidenceQuality, EvidenceStatus, MachineIdentityDecision,
    ReadOutcome, decide_machine_identity, evaluate_slot,
};
use procfs::{ProcError, process::MountInfo, process::Process};
use raw_cpuid::CpuId;
use smbioslib::{
    SMBiosBaseboardInformation, SMBiosData, SMBiosEntryPoint32, SMBiosEntryPoint64,
    SMBiosSystemInformation, SMBiosVersion, SystemUuidData,
};
use uuid::Uuid;
use zeroize::Zeroize as _;

const DMI_DIRECTORY: &str = "/sys/class/dmi/id";
const DMI_SYSTEM_UUID: &str = "/sys/class/dmi/id/product_uuid";
const DMI_BOARD_SERIAL: &str = "/sys/class/dmi/id/board_serial";
const SMBIOS_DIRECTORY: &str = "/sys/firmware/dmi/tables";
const SMBIOS_ENTRY_POINT: &str = "/sys/firmware/dmi/tables/smbios_entry_point";
const SMBIOS_TABLE: &str = "/sys/firmware/dmi/tables/DMI";
const SYS_DEV_BLOCK: &str = "/sys/dev/block";
const SYS_VIRTUAL_BLOCK: &str = "/sys/devices/virtual/block";
const UDEV_DATA_DIRECTORY: &str = "/run/udev/data";

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceStatus {
    Unavailable,
    PermissionDenied,
    Unsupported,
}

enum SmbiosReadings {
    Values {
        system_uuid: Option<String>,
        board_serial: Option<String>,
    },
    Status(SourceStatus),
}

/// Collects the three frozen hardware readings from a filesystem rooted at `filesystem_root`.
/// Production passes `/`; tests pass an isolated fixture root.
#[must_use]
pub fn collect(filesystem_root: &Path) -> [ReadOutcome; 3] {
    let [system_uuid, board_serial] = collect_dmi(filesystem_root);
    processor_serial_conflict_check();
    let disk_serial = match Process::myself().and_then(|process| process.mountinfo()) {
        Ok(mounts) => first_disk_serial(filesystem_root, &mounts.0),
        Err(error) => outcome_from_status(proc_error_status(&error)),
    };
    [system_uuid, board_serial, disk_serial]
}

pub(crate) fn collect_with_mountinfo(
    filesystem_root: &Path,
    mountinfo: &[MountInfo],
) -> [ReadOutcome; 3] {
    let [system_uuid, board_serial] = collect_dmi(filesystem_root);
    processor_serial_conflict_check();
    let disk_serial = first_disk_serial(filesystem_root, mountinfo);
    [system_uuid, board_serial, disk_serial]
}

/// Runs collection and the frozen pure derivation pipeline without exposing normalized values.
#[must_use]
pub fn derive_claim(filesystem_root: &Path, fleet_namespace: Uuid) -> SanitizedHardwareClaim {
    claim_from_readings(collect(filesystem_root), fleet_namespace)
}

pub(crate) fn derive_claim_with_mountinfo(
    filesystem_root: &Path,
    mountinfo: &[MountInfo],
    fleet_namespace: Uuid,
) -> SanitizedHardwareClaim {
    claim_from_readings(
        collect_with_mountinfo(filesystem_root, mountinfo),
        fleet_namespace,
    )
}

fn rooted(filesystem_root: &Path, absolute_path: &str) -> PathBuf {
    let relative = Path::new(absolute_path)
        .strip_prefix("/")
        .unwrap_or_else(|_| Path::new(absolute_path));
    filesystem_root.join(relative)
}

fn interface_status(path: &Path) -> Result<(), SourceStatus> {
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

fn io_status(error: &io::Error) -> SourceStatus {
    match error.kind() {
        ErrorKind::PermissionDenied => SourceStatus::PermissionDenied,
        ErrorKind::Unsupported => SourceStatus::Unsupported,
        _ => SourceStatus::Unavailable,
    }
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, SourceStatus> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.permissions().mode() & 0o444 == 0 => {
            return Err(SourceStatus::PermissionDenied);
        }
        Ok(_) => {}
        Err(error) => return Err(io_status(&error)),
    }
    fs::read(path).map_err(|error| io_status(&error))
}

fn bytes_as_value(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(value) => value,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    }
}

fn read_value(path: &Path) -> ReadOutcome {
    match read_bytes(path) {
        Ok(bytes) => ReadOutcome::Value(bytes_as_value(bytes)),
        Err(status) => outcome_from_status(status),
    }
}

fn outcome_from_status(status: SourceStatus) -> ReadOutcome {
    match status {
        SourceStatus::Unavailable => ReadOutcome::Unavailable,
        SourceStatus::PermissionDenied => ReadOutcome::PermissionDenied,
        SourceStatus::Unsupported => ReadOutcome::Unsupported,
    }
}

fn primary_dmi_readings(filesystem_root: &Path) -> [ReadOutcome; 2] {
    match interface_status(&rooted(filesystem_root, DMI_DIRECTORY)) {
        Ok(()) => [
            read_value(&rooted(filesystem_root, DMI_SYSTEM_UUID)),
            read_value(&rooted(filesystem_root, DMI_BOARD_SERIAL)),
        ],
        Err(status) => [outcome_from_status(status), outcome_from_status(status)],
    }
}

fn smbios_version(entry_point: Vec<u8>) -> Result<SMBiosVersion, SourceStatus> {
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

fn smbios_readings(filesystem_root: &Path) -> SmbiosReadings {
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

fn comparison_characters(value: &str) -> impl Iterator<Item = char> + '_ {
    value
        .trim_matches(|character: char| character.is_ascii_whitespace())
        .chars()
        .filter(|character| !matches!(character, '-' | '_' | ':' | ' '))
        .flat_map(char::to_lowercase)
}

fn values_are_equivalent(left: &str, right: &str) -> bool {
    comparison_characters(left).eq(comparison_characters(right))
}

fn merge_dmi(primary: ReadOutcome, fallback: Option<String>) -> ReadOutcome {
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

fn apply_smbios_failure(primary: ReadOutcome, smbios_status: SourceStatus) -> ReadOutcome {
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

fn collect_dmi(filesystem_root: &Path) -> [ReadOutcome; 2] {
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

/// Processor Serial Number is not one of the three frozen slots. Until a PSN-capable fixture
/// defines a cross-source comparison, this seam only verifies the real feature bit before reading
/// leaf 0x03, then zeroizes the result without changing any slot or fabricating evidence.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn processor_serial_conflict_check() {
    let cpuid = CpuId::new();
    let psn_is_real = cpuid
        .get_feature_info()
        .is_some_and(|features| features.has_psn());
    if psn_is_real && let Some(serial) = cpuid.get_processor_serial() {
        let mut bytes = serial.serial_all().to_ne_bytes();
        bytes.zeroize();
    }
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn processor_serial_conflict_check() {}

fn proc_error_status(error: &ProcError) -> SourceStatus {
    match error {
        ProcError::PermissionDenied(_) => SourceStatus::PermissionDenied,
        ProcError::NotFound(_) => SourceStatus::Unsupported,
        ProcError::Incomplete(_)
        | ProcError::Io(_, _)
        | ProcError::Other(_)
        | ProcError::InternalError(_) => SourceStatus::Unavailable,
    }
}

fn parse_device_number(value: &str) -> Option<String> {
    let (major, minor) = value.split_once(':')?;
    if major.is_empty() || minor.is_empty() || minor.contains(':') {
        return None;
    }
    let major = major.parse::<u32>().ok()?;
    let minor = minor.parse::<u32>().ok()?;
    Some(format!("{major}:{minor}"))
}

fn metadata_is_file(path: &Path) -> Result<bool, SourceStatus> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_status(&error)),
    }
}

fn resolve_block_path(
    filesystem_root: &Path,
    block_path: &Path,
    visited: &mut HashSet<PathBuf>,
    whole_disks: &mut BTreeSet<String>,
) -> Result<(), SourceStatus> {
    let canonical = fs::canonicalize(block_path).map_err(|error| io_status(&error))?;
    if !visited.insert(canonical.clone()) {
        // Two slaves of one mapper device may share a whole disk (one-disk LVM with
        // several PVs). The disk set deduplicates by device number, so a revisit is
        // convergence, not ambiguity; cycles still terminate because nothing recurses.
        return Ok(());
    }

    if metadata_is_file(&canonical.join("partition"))? {
        let parent = canonical.parent().ok_or(SourceStatus::Unavailable)?;
        return resolve_block_path(filesystem_root, parent, visited, whole_disks);
    }

    let slaves_path = canonical.join("slaves");
    match fs::read_dir(&slaves_path) {
        Ok(entries) => {
            let mut found_slave = false;
            for entry in entries {
                let entry = entry.map_err(|error| io_status(&error))?;
                found_slave = true;
                resolve_block_path(filesystem_root, &entry.path(), visited, whole_disks)?;
            }
            if found_slave {
                return Ok(());
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(io_status(&error)),
    }

    if canonical.starts_with(rooted(filesystem_root, SYS_VIRTUAL_BLOCK)) {
        return Err(SourceStatus::Unavailable);
    }

    let mut dev_value = bytes_as_value(read_bytes(&canonical.join("dev"))?);
    let dev_number = parse_device_number(dev_value.trim()).ok_or(SourceStatus::Unavailable)?;
    dev_value.zeroize();
    whole_disks.insert(dev_number);
    Ok(())
}

fn resolve_whole_disks(
    filesystem_root: &Path,
    root_device_number: &str,
) -> Result<BTreeSet<String>, SourceStatus> {
    interface_status(&rooted(filesystem_root, SYS_DEV_BLOCK))?;
    let mut whole_disks = BTreeSet::new();
    let mut visited = HashSet::new();
    resolve_block_path(
        filesystem_root,
        &rooted(
            filesystem_root,
            &format!("{SYS_DEV_BLOCK}/{root_device_number}"),
        ),
        &mut visited,
        &mut whole_disks,
    )?;
    Ok(whole_disks)
}

fn udev_property(data: &[u8], name: &[u8]) -> Option<Vec<u8>> {
    data.split(|byte| *byte == b'\n').find_map(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        line.strip_prefix(name)
            .filter(|value| !value.is_empty())
            .map(<[u8]>::to_vec)
    })
}

fn udev_disk_serial(filesystem_root: &Path, device_number: &str) -> ReadOutcome {
    if let Err(status) = interface_status(&rooted(filesystem_root, UDEV_DATA_DIRECTORY)) {
        return outcome_from_status(status);
    }
    let mut data = match read_bytes(&rooted(
        filesystem_root,
        &format!("{UDEV_DATA_DIRECTORY}/b{device_number}"),
    )) {
        Ok(bytes) => bytes,
        Err(status) => return outcome_from_status(status),
    };
    let serial = udev_property(&data, b"E:ID_SERIAL_SHORT=")
        .or_else(|| udev_property(&data, b"E:ID_SERIAL="));
    data.zeroize();
    serial.map_or(ReadOutcome::Unavailable, |bytes| {
        ReadOutcome::Value(bytes_as_value(bytes))
    })
}

fn first_disk_serial(filesystem_root: &Path, mountinfo: &[MountInfo]) -> ReadOutcome {
    let root_mounts = mountinfo
        .iter()
        .filter(|mount| mount.mount_point == Path::new("/"))
        .collect::<Vec<_>>();
    let [root_mount] = root_mounts.as_slice() else {
        return ReadOutcome::Unavailable;
    };
    if root_mount.fs_type == "overlay" {
        return ReadOutcome::Unavailable;
    }
    let Some(root_device_number) = parse_device_number(&root_mount.majmin) else {
        return ReadOutcome::Unavailable;
    };
    let whole_disks = match resolve_whole_disks(filesystem_root, &root_device_number) {
        Ok(disks) => disks,
        Err(status) => return outcome_from_status(status),
    };
    let mut disks = whole_disks.into_iter();
    let Some(whole_disk) = disks.next() else {
        return ReadOutcome::Unavailable;
    };
    if disks.next().is_some() {
        return ReadOutcome::Unavailable;
    }
    udev_disk_serial(filesystem_root, &whole_disk)
}

fn quality_label(quality: EvidenceQuality) -> &'static str {
    match quality {
        EvidenceQuality::Weak => "weak",
        EvidenceQuality::Medium => "medium",
        EvidenceQuality::Strong => "strong",
    }
}

fn zeroize_readings(readings: &mut [ReadOutcome; 3]) {
    for reading in readings {
        match reading {
            ReadOutcome::Value(value) => value.zeroize(),
            ReadOutcome::Unavailable | ReadOutcome::PermissionDenied | ReadOutcome::Unsupported => {
            }
        }
    }
}

fn claim_from_readings(
    mut readings: [ReadOutcome; 3],
    fleet_namespace: Uuid,
) -> SanitizedHardwareClaim {
    let evaluations = std::array::from_fn(|index| {
        evaluate_slot(ANCHOR_ORDER[index], &readings[index], fleet_namespace)
    });
    let decision = decide_machine_identity(&evaluations);
    zeroize_readings(&mut readings);

    let candidates = evaluations
        .iter()
        .enumerate()
        .filter_map(|(index, evaluation)| {
            if evaluation.status != EvidenceStatus::Present {
                return None;
            }
            evaluation
                .candidate_id
                .map(|candidate_id| HardwareCandidate {
                    anchor_kind: ANCHOR_ORDER[index].label().to_owned(),
                    candidate_id: candidate_id.to_string(),
                    quality: quality_label(evaluation.quality).to_owned(),
                })
        })
        .collect();
    let collection_complete =
        decision.collection_completeness() == CollectionCompleteness::Complete;
    let (decision, machine_hardware_id, present_slot_count) = match decision {
        MachineIdentityDecision::Derived {
            machine_hardware_id,
            present_slot_count,
        } => (
            "derived".to_owned(),
            Some(machine_hardware_id.to_string()),
            u32::try_from(present_slot_count).unwrap_or(u32::MAX),
        ),
        MachineIdentityDecision::InsufficientSources { present_slot_count } => (
            "insufficient_sources".to_owned(),
            None,
            u32::try_from(present_slot_count).unwrap_or(u32::MAX),
        ),
        MachineIdentityDecision::Unsupported { present_slot_count } => (
            "unsupported".to_owned(),
            None,
            u32::try_from(present_slot_count).unwrap_or(u32::MAX),
        ),
    };

    SanitizedHardwareClaim {
        candidates,
        collection_complete,
        decision,
        machine_hardware_id,
        present_slot_count,
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::symlink, str::FromStr as _};

    use natsume_machine_identity::{AnchorKind, EvidenceStatus, evaluate_slot};
    use tempfile::TempDir;

    use super::*;

    const TEST_NAMESPACE: Uuid = Uuid::from_u128(0x1234_5678_1234_5678_9234_5678_1234_5678);
    const SYSTEM_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn tempdir() -> TempDir {
        match TempDir::new() {
            Ok(directory) => directory,
            Err(error) => panic!("fixture directory must be created: {error}"),
        }
    }

    fn fixture_path(root: &Path, absolute: &str) -> PathBuf {
        rooted(root, absolute)
    }

    fn write_fixture(root: &Path, absolute: &str, bytes: &[u8]) {
        let path = fixture_path(root, absolute);
        let Some(parent) = path.parent() else {
            panic!("fixture path must have a parent");
        };
        if let Err(error) = fs::create_dir_all(parent) {
            panic!("fixture parent must be created: {error}");
        }
        if let Err(error) = fs::write(path, bytes) {
            panic!("fixture file must be written: {error}");
        }
    }

    fn mount(line: &str) -> MountInfo {
        match MountInfo::from_line(line) {
            Ok(mount) => mount,
            Err(error) => panic!("mountinfo fixture must parse: {error}"),
        }
    }

    fn uuid_smbios_bytes(uuid: Uuid) -> [u8; 16] {
        let (time_low, time_mid, time_high, remainder) = uuid.as_fields();
        let mut bytes = [0_u8; 16];
        bytes[0..4].copy_from_slice(&time_low.to_le_bytes());
        bytes[4..6].copy_from_slice(&time_mid.to_le_bytes());
        bytes[6..8].copy_from_slice(&time_high.to_le_bytes());
        bytes[8..16].copy_from_slice(remainder);
        bytes
    }

    fn smbios_entry_point() -> Vec<u8> {
        let mut entry = vec![
            b'_', b'S', b'M', b'3', b'_', 0, 0x18, 3, 9, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0,
        ];
        let checksum = entry.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
        entry[5] = 0_u8.wrapping_sub(checksum);
        entry
    }

    fn smbios_table(system_uuid: &str, board_serial: &str) -> Vec<u8> {
        let Ok(uuid) = Uuid::from_str(system_uuid) else {
            panic!("fixture UUID must parse");
        };
        let mut table = vec![1, 0x19, 1, 0, 1, 2, 3, 4];
        table.extend_from_slice(&uuid_smbios_bytes(uuid));
        table.push(0);
        table.extend_from_slice(
            b"Fixture Vendor\0Fixture Product\0Fixture Version\0System Serial\0\0",
        );
        table.extend_from_slice(&[2, 0x0f, 2, 0, 1, 2, 3, 4, 5, 0, 0xff, 0xff, 10, 0, 0]);
        table.extend_from_slice(b"Fixture Vendor\0Fixture Board\0Fixture Version\0");
        table.extend_from_slice(board_serial.as_bytes());
        table.extend_from_slice(b"\0Fixture Asset\0\0");
        table.extend_from_slice(&[127, 4, 0xff, 0xff, 0, 0]);
        table
    }

    fn install_smbios(root: &Path, system_uuid: &str, board_serial: &str) {
        write_fixture(root, SMBIOS_ENTRY_POINT, &smbios_entry_point());
        write_fixture(root, SMBIOS_TABLE, &smbios_table(system_uuid, board_serial));
    }

    fn install_dmi(root: &Path, system_uuid: &[u8], board_serial: &[u8]) {
        write_fixture(root, DMI_SYSTEM_UUID, system_uuid);
        write_fixture(root, DMI_BOARD_SERIAL, board_serial);
    }

    fn install_partition_disk(root: &Path, serial: &[u8]) {
        let partition = fixture_path(root, "/sys/devices/pci/block/sda/sda2");
        if let Err(error) = fs::create_dir_all(&partition) {
            panic!("partition fixture must be created: {error}");
        }
        write_fixture(root, "/sys/devices/pci/block/sda/sda2/partition", b"2\n");
        write_fixture(root, "/sys/devices/pci/block/sda/dev", b"8:0\n");
        let dev_block = fixture_path(root, SYS_DEV_BLOCK);
        if let Err(error) = fs::create_dir_all(&dev_block) {
            panic!("sysfs block fixture must be created: {error}");
        }
        if let Err(error) = symlink("../../devices/pci/block/sda/sda2", dev_block.join("8:2")) {
            panic!("sysfs block symlink must be created: {error}");
        }
        let mut udev = b"E:ID_SERIAL=vendor-long-value\nE:ID_SERIAL_SHORT=".to_vec();
        udev.extend_from_slice(serial);
        udev.push(b'\n');
        write_fixture(root, "/run/udev/data/b8:0", &udev);
    }

    fn assert_value(reading: &ReadOutcome, expected: &str) {
        match reading {
            ReadOutcome::Value(value) => assert_eq!(value.trim(), expected),
            ReadOutcome::Unavailable => panic!("expected a value, got unavailable"),
            ReadOutcome::PermissionDenied => panic!("expected a value, got permission denied"),
            ReadOutcome::Unsupported => panic!("expected a value, got unsupported"),
        }
    }

    #[test]
    fn both_sysfs_dmi_values_are_primary() {
        let fixture = tempdir();
        install_dmi(fixture.path(), SYSTEM_UUID.as_bytes(), b"board-42\n");
        let readings = collect_with_mountinfo(fixture.path(), &[]);

        assert_value(&readings[0], SYSTEM_UUID);
        assert_value(&readings[1], "board-42");
    }

    #[test]
    fn smbios_fills_a_missing_sysfs_value_from_byte_fixtures() {
        let fixture = tempdir();
        write_fixture(fixture.path(), DMI_BOARD_SERIAL, b"board-42\n");
        install_smbios(fixture.path(), SYSTEM_UUID, "board-42");
        let readings = collect_with_mountinfo(fixture.path(), &[]);

        assert_value(&readings[0], SYSTEM_UUID);
        assert_value(&readings[1], "board-42");
    }

    #[test]
    fn sysfs_smbios_conflict_is_conservatively_unavailable() {
        let fixture = tempdir();
        install_dmi(fixture.path(), SYSTEM_UUID.as_bytes(), b"board-sysfs\n");
        install_smbios(fixture.path(), SYSTEM_UUID, "board-smbios");
        let readings = collect_with_mountinfo(fixture.path(), &[]);

        assert!(matches!(readings[1], ReadOutcome::Unavailable));
        let evaluation = evaluate_slot(ANCHOR_ORDER[1], &readings[1], TEST_NAMESPACE);
        assert_eq!(evaluation.status, EvidenceStatus::Unavailable);
    }

    #[test]
    fn permission_denied_is_preserved() {
        let fixture = tempdir();
        install_dmi(fixture.path(), SYSTEM_UUID.as_bytes(), b"board-42\n");
        let product_uuid = fixture_path(fixture.path(), DMI_SYSTEM_UUID);
        if let Err(error) = fs::set_permissions(&product_uuid, fs::Permissions::from_mode(0o000)) {
            panic!("fixture mode must be changed: {error}");
        }
        let readings = collect_with_mountinfo(fixture.path(), &[]);

        assert!(matches!(readings[0], ReadOutcome::PermissionDenied));
    }

    #[test]
    fn placeholder_reaches_the_pure_rejection_policy() {
        let fixture = tempdir();
        install_dmi(
            fixture.path(),
            SYSTEM_UUID.as_bytes(),
            b"To Be Filled By OEM\n",
        );
        let readings = collect_with_mountinfo(fixture.path(), &[]);
        let evaluation = evaluate_slot(ANCHOR_ORDER[1], &readings[1], TEST_NAMESPACE);

        assert_eq!(evaluation.status, EvidenceStatus::RejectedPlaceholder);
    }

    #[test]
    fn root_partition_resolves_to_parent_whole_disk_and_short_udev_serial() {
        let fixture = tempdir();
        install_partition_disk(fixture.path(), b"disk-99");
        let mountinfo = [mount("36 25 8:2 / / rw,relatime - ext4 /dev/sda2 rw")];
        let readings = collect_with_mountinfo(fixture.path(), &mountinfo);

        assert_value(&readings[2], "disk-99");
    }

    #[test]
    fn ambiguous_root_mount_is_unavailable() {
        let fixture = tempdir();
        install_partition_disk(fixture.path(), b"disk-99");
        let mountinfo = [
            mount("36 25 8:2 / / rw,relatime - ext4 /dev/sda2 rw"),
            mount("40 25 8:3 / / rw,relatime - ext4 /dev/sda3 rw"),
        ];
        let readings = collect_with_mountinfo(fixture.path(), &mountinfo);

        assert!(matches!(readings[2], ReadOutcome::Unavailable));
    }

    #[test]
    fn one_disk_lvm_with_two_slave_partitions_resolves_the_shared_whole_disk() {
        let fixture = tempdir();
        write_fixture(fixture.path(), "/sys/devices/pci/block/sda/dev", b"8:0\n");
        for partition in ["sda2", "sda3"] {
            write_fixture(
                fixture.path(),
                &format!("/sys/devices/pci/block/sda/{partition}/partition"),
                b"1\n",
            );
        }
        let slaves = fixture_path(fixture.path(), "/sys/devices/virtual/block/dm-0/slaves");
        if let Err(error) = fs::create_dir_all(&slaves) {
            panic!("device-mapper fixture must be created: {error}");
        }
        for partition in ["sda2", "sda3"] {
            if let Err(error) = symlink(
                format!("../../../../pci/block/sda/{partition}"),
                slaves.join(partition),
            ) {
                panic!("slave symlink must be created: {error}");
            }
        }
        let dev_block = fixture_path(fixture.path(), SYS_DEV_BLOCK);
        if let Err(error) = fs::create_dir_all(&dev_block) {
            panic!("sysfs block fixture must be created: {error}");
        }
        if let Err(error) = symlink("../../devices/virtual/block/dm-0", dev_block.join("253:0")) {
            panic!("device-mapper block symlink must be created: {error}");
        }
        write_fixture(
            fixture.path(),
            "/run/udev/data/b8:0",
            b"E:ID_SERIAL_SHORT=lvm-disk-7\n",
        );
        let mountinfo = [mount(
            "36 25 253:0 / / rw,relatime - ext4 /dev/mapper/root rw",
        )];
        let readings = collect_with_mountinfo(fixture.path(), &mountinfo);

        assert_value(&readings[2], "lvm-disk-7");
    }

    #[test]
    fn root_spanning_two_whole_disks_is_unavailable() {
        let fixture = tempdir();
        for (disk, dev) in [("sda", "8:0"), ("sdb", "8:16")] {
            let disk_path = fixture_path(fixture.path(), &format!("/sys/devices/pci/block/{disk}"));
            if let Err(error) = fs::create_dir_all(&disk_path) {
                panic!("disk fixture must be created: {error}");
            }
            write_fixture(
                fixture.path(),
                &format!("/sys/devices/pci/block/{disk}/dev"),
                format!("{dev}\n").as_bytes(),
            );
        }
        let slaves = fixture_path(fixture.path(), "/sys/devices/virtual/block/dm-0/slaves");
        if let Err(error) = fs::create_dir_all(&slaves) {
            panic!("device-mapper fixture must be created: {error}");
        }
        for disk in ["sda", "sdb"] {
            if let Err(error) = symlink(format!("../../../../pci/block/{disk}"), slaves.join(disk))
            {
                panic!("slave symlink must be created: {error}");
            }
        }
        let dev_block = fixture_path(fixture.path(), SYS_DEV_BLOCK);
        if let Err(error) = fs::create_dir_all(&dev_block) {
            panic!("sysfs block fixture must be created: {error}");
        }
        if let Err(error) = symlink("../../devices/virtual/block/dm-0", dev_block.join("253:0")) {
            panic!("device-mapper block symlink must be created: {error}");
        }
        let mountinfo = [mount(
            "36 25 253:0 / / rw,relatime - ext4 /dev/mapper/root rw",
        )];
        let readings = collect_with_mountinfo(fixture.path(), &mountinfo);

        assert!(matches!(readings[2], ReadOutcome::Unavailable));
    }

    #[test]
    fn fixture_collection_derives_the_wp1_golden_machine_id() {
        let fixture = tempdir();
        install_dmi(
            fixture.path(),
            b" 550E8400-E29B-41D4-A716-446655440000 \n",
            b" BOARD-42 \n",
        );
        install_partition_disk(fixture.path(), b"DISK_99");
        let mountinfo = [mount("36 25 8:2 / / rw,relatime - ext4 /dev/sda2 rw")];
        let claim = derive_claim_with_mountinfo(fixture.path(), &mountinfo, TEST_NAMESPACE);

        assert_eq!(claim.decision, "derived");
        assert_eq!(
            claim.machine_hardware_id.as_deref(),
            Some("a9aa9d04-3ece-5567-8260-910930ff5e03")
        );
        assert_eq!(claim.present_slot_count, 3);
        assert!(claim.collection_complete);
        assert_eq!(claim.candidates.len(), 3);
    }

    #[test]
    fn absent_platform_interfaces_are_unsupported() {
        let fixture = tempdir();
        let readings = collect_with_mountinfo(
            fixture.path(),
            &[mount("36 25 8:2 / / rw,relatime - ext4 /dev/sda2 rw")],
        );

        assert!(matches!(readings[0], ReadOutcome::Unsupported));
        assert!(matches!(readings[1], ReadOutcome::Unsupported));
        assert!(matches!(readings[2], ReadOutcome::Unsupported));
    }

    #[test]
    fn frozen_slot_labels_are_used_for_candidates() {
        let claim = claim_from_readings(
            [
                ReadOutcome::Value(SYSTEM_UUID.to_owned()),
                ReadOutcome::Value("board-42".to_owned()),
                ReadOutcome::Unavailable,
            ],
            TEST_NAMESPACE,
        );

        assert_eq!(claim.candidates.len(), 2);
        assert_eq!(
            claim
                .candidates
                .iter()
                .map(|candidate| candidate.anchor_kind.as_str())
                .collect::<Vec<_>>(),
            [
                AnchorKind::DmiSystemUuid.label(),
                AnchorKind::DmiBoardSerial.label()
            ]
        );
    }
}
