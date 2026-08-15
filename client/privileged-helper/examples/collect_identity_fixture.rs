// Development-only anonymized hardware fixture collector; never packaged.

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, ErrorKind, Write as _},
    path::Path,
    process::ExitCode,
};

use natsume_machine_identity::{
    ANCHOR_ORDER, CollectionCompleteness, EvidenceQuality, EvidenceStatus, ReadOutcome,
    collection_completeness, evaluate_slot,
};
use procfs::process::Process;
use serde::Serialize;
use udev::{Device, DeviceType};

const USAGE: &str = "usage: collect_identity_fixture --namespace <uuid>";
const DMI_ID_DIRECTORY: &str = "/sys/class/dmi/id";
const DMI_SYSTEM_UUID: &str = "/sys/class/dmi/id/product_uuid";
const DMI_BOARD_SERIAL: &str = "/sys/class/dmi/id/board_serial";

#[derive(Serialize)]
struct FixtureSlot {
    anchor_kind: &'static str,
    status: EvidenceStatus,
    quality: EvidenceQuality,
    candidate_id: Option<String>,
}

#[derive(Serialize)]
struct FixtureRecord {
    slots: [FixtureSlot; 3],
    completeness: CollectionCompleteness,
}

fn write_usage() {
    let _write_result = writeln!(io::stderr().lock(), "{USAGE}");
}

fn write_output_error() {
    let _write_result = writeln!(
        io::stderr().lock(),
        "collect_identity_fixture: failed to write JSON output"
    );
}

fn namespace_argument() -> Option<OsString> {
    let mut arguments = env::args_os().skip(1);
    match (arguments.next(), arguments.next(), arguments.next()) {
        (Some(flag), Some(namespace), None) if flag == OsStr::new("--namespace") => Some(namespace),
        _ => None,
    }
}

fn map_io_error(error: &io::Error) -> ReadOutcome {
    match error.kind() {
        ErrorKind::PermissionDenied => ReadOutcome::PermissionDenied,
        ErrorKind::Unsupported => ReadOutcome::Unsupported,
        // This includes ErrorKind::NotFound (the ENOENT class) and other transient I/O errors.
        _ => ReadOutcome::Unavailable,
    }
}

fn bytes_as_value(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(value) => value,
        Err(error) => {
            // ReadOutcome has no encoding-error variant. Lossy conversion preserves a U+FFFD
            // marker, which the pure evaluator classifies as malformed instead of deriving an ID.
            String::from_utf8_lossy(error.as_bytes()).into_owned()
        }
    }
}

fn read_value(path: &Path) -> ReadOutcome {
    match fs::read(path) {
        Ok(bytes) => ReadOutcome::Value(bytes_as_value(bytes)),
        Err(error) => map_io_error(&error),
    }
}

fn dmi_readings() -> [ReadOutcome; 2] {
    // A missing DMI directory means the platform does not expose DMI at all. Once the directory
    // exists, a missing individual attribute is an unavailable reading rather than unsupported.
    match fs::metadata(DMI_ID_DIRECTORY) {
        Ok(metadata) if metadata.is_dir() => [
            read_value(Path::new(DMI_SYSTEM_UUID)),
            read_value(Path::new(DMI_BOARD_SERIAL)),
        ],
        Ok(_) => [ReadOutcome::Unsupported, ReadOutcome::Unsupported],
        Err(error) if error.kind() == ErrorKind::NotFound => {
            [ReadOutcome::Unsupported, ReadOutcome::Unsupported]
        }
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            [ReadOutcome::PermissionDenied, ReadOutcome::PermissionDenied]
        }
        Err(_) => [ReadOutcome::Unavailable, ReadOutcome::Unavailable],
    }
}

fn parse_device_number(value: &str) -> Option<(u32, u32)> {
    let (major, minor) = value.split_once(':')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

fn is_virtual_block_syspath(path: &Path) -> bool {
    path.starts_with("/sys/devices/virtual/block")
}

fn whole_disk_device(device: Device) -> Option<Device> {
    let is_disk = device.devtype() == Some(OsStr::new("disk"));
    let is_partition = device.devtype() == Some(OsStr::new("partition"));
    let disk = if is_disk {
        device
    } else if is_partition {
        device
            .parent_with_subsystem_devtype("block", "disk")
            .ok()
            .flatten()?
    } else {
        return None;
    };

    (disk.subsystem() == Some(OsStr::new("block"))
        && disk.devtype() == Some(OsStr::new("disk"))
        && !is_virtual_block_syspath(disk.syspath())
        && disk.property_value("DM_UUID").is_none()
        && disk.property_value("MD_UUID").is_none())
    .then_some(disk)
}

fn root_whole_disk_device() -> Option<Device> {
    let process = Process::myself().ok()?;
    let mounts = process.mountinfo().ok()?;
    let root_mount = mounts
        .into_iter()
        .rev()
        .find(|mount| mount.mount_point == Path::new("/"))?;
    if root_mount.fs_type == "overlay" {
        return None;
    }
    let (major, minor) = parse_device_number(&root_mount.majmin)?;
    let device = Device::from_devnum(DeviceType::Block, rustix::fs::makedev(major, minor)).ok()?;
    whole_disk_device(device)
}

fn disk_serial(device: &Device) -> Option<OsString> {
    device
        .property_value("ID_SERIAL_SHORT")
        .or_else(|| device.attribute_value("serial"))
        .or_else(|| device.attribute_value("device/serial"))
        .map(OsStr::to_owned)
}

fn first_disk_serial() -> ReadOutcome {
    let Some(device) = root_whole_disk_device() else {
        return ReadOutcome::Unavailable;
    };
    let Some(serial) = disk_serial(&device) else {
        return ReadOutcome::Unavailable;
    };
    match serial.into_string() {
        Ok(value) => ReadOutcome::Value(value),
        Err(value) => ReadOutcome::Value(value.to_string_lossy().into_owned()),
    }
}

fn main() -> ExitCode {
    let Some(namespace_argument) = namespace_argument() else {
        write_usage();
        return ExitCode::from(2);
    };
    let Ok(namespace_text) = namespace_argument.into_string() else {
        write_usage();
        return ExitCode::from(2);
    };
    let Ok(fleet_namespace) = namespace_text.parse() else {
        write_usage();
        return ExitCode::from(2);
    };

    let [system_uuid, board_serial] = dmi_readings();
    let readings = [system_uuid, board_serial, first_disk_serial()];
    let evaluations = [
        evaluate_slot(ANCHOR_ORDER[0], &readings[0], fleet_namespace),
        evaluate_slot(ANCHOR_ORDER[1], &readings[1], fleet_namespace),
        evaluate_slot(ANCHOR_ORDER[2], &readings[2], fleet_namespace),
    ];
    let statuses = [
        evaluations[0].status,
        evaluations[1].status,
        evaluations[2].status,
    ];
    let slots = std::array::from_fn(|index| FixtureSlot {
        anchor_kind: ANCHOR_ORDER[index].label(),
        status: evaluations[index].status,
        quality: evaluations[index].quality,
        candidate_id: evaluations[index]
            .candidate_id
            .map(|candidate| candidate.to_string()),
    });
    let record = FixtureRecord {
        slots,
        completeness: collection_completeness(&statuses),
    };

    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &record).is_err() || writeln!(stdout).is_err() {
        write_output_error();
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_numbers_are_parsed_without_parsing_mountinfo_text() {
        assert_eq!(parse_device_number("8:2"), Some((8, 2)));
        assert_eq!(parse_device_number("259:12"), Some((259, 12)));
        assert_eq!(parse_device_number("not-a-device"), None);
        assert_eq!(parse_device_number("8:2:1"), None);
    }

    #[test]
    fn physical_disk_paths_are_distinct_from_virtual_block_stacks() {
        for path in [
            "/sys/devices/pci0000:00/host0/target0:0:0/0:0:0:0/block/sda",
            "/sys/devices/pci0000:00/nvme/nvme0/nvme0n1",
            "/sys/devices/pci0000:00/virtio1/block/vda",
        ] {
            assert!(!is_virtual_block_syspath(Path::new(path)));
        }
        for path in [
            "/sys/devices/virtual/block/dm-0",
            "/sys/devices/virtual/block/md0",
            "/sys/devices/virtual/block/loop0",
        ] {
            assert!(is_virtual_block_syspath(Path::new(path)));
        }
    }
}
