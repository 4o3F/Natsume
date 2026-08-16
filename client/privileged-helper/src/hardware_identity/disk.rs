use std::{
    collections::{BTreeSet, HashSet},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use natsume_machine_identity::ReadOutcome;
use procfs::{ProcError, process::MountInfo};
use zeroize::Zeroize as _;

use super::{
    SYS_DEV_BLOCK, SYS_VIRTUAL_BLOCK, SourceStatus, UDEV_DATA_DIRECTORY,
    source::{
        bytes_as_value, interface_status, io_status, outcome_from_status, read_bytes, rooted,
    },
};

pub(super) fn proc_error_status(error: &ProcError) -> SourceStatus {
    match error {
        ProcError::PermissionDenied(_) => SourceStatus::PermissionDenied,
        ProcError::NotFound(_) => SourceStatus::Unsupported,
        ProcError::Incomplete(_)
        | ProcError::Io(_, _)
        | ProcError::Other(_)
        | ProcError::InternalError(_) => SourceStatus::Unavailable,
    }
}

pub(super) fn parse_device_number(value: &str) -> Option<String> {
    let (major, minor) = value.split_once(':')?;
    if major.is_empty() || minor.is_empty() || minor.contains(':') {
        return None;
    }
    let major = major.parse::<u32>().ok()?;
    let minor = minor.parse::<u32>().ok()?;
    Some(format!("{major}:{minor}"))
}

pub(super) fn metadata_is_file(path: &Path) -> Result<bool, SourceStatus> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_status(&error)),
    }
}

pub(super) fn resolve_block_path(
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

pub(super) fn resolve_whole_disks(
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

pub(super) fn udev_property(data: &[u8], name: &[u8]) -> Option<Vec<u8>> {
    data.split(|byte| *byte == b'\n').find_map(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        line.strip_prefix(name)
            .filter(|value| !value.is_empty())
            .map(<[u8]>::to_vec)
    })
}

pub(super) fn udev_disk_serial(filesystem_root: &Path, device_number: &str) -> ReadOutcome {
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

pub(super) fn first_disk_serial(filesystem_root: &Path, mountinfo: &[MountInfo]) -> ReadOutcome {
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
