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
        b'_', b'S', b'M', b'3', b'_', 0, 0x18, 3, 9, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
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
    table.extend_from_slice(b"Fixture Vendor\0Fixture Product\0Fixture Version\0System Serial\0\0");
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
        if let Err(error) = symlink(format!("../../../../pci/block/{disk}"), slaves.join(disk)) {
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
