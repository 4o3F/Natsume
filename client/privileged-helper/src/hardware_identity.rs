//! Linux hardware collection and identity policy belong to the privileged helper.
//!
//! Collection order:
//! 1. DMI system UUID and motherboard serial from `/sys/class/dmi/id`.
//! 2. `smbios-lib` only to fill missing DMI values and cross-check present values.
//! 3. `procfs::process::MountInfo` plus sysfs and udev data for one root whole disk.
//!
//! No app-local or `/etc/machine-id` fallback is accepted because it can be copied with the disk.
//! Raw serials are normalized and hashed in this process, then zeroized or dropped. No shell
//! commands and no text parsing of dmidecode, lsblk, udevadm, or findmnt output are allowed.

use std::path::Path;

use natsume_local_control_api::{
    DerivedMachineIdentity, MachineIdentityError, MachineIdentityQuality,
};
use procfs::process::Process;
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

mod disk;
mod dmi;
mod policy;
mod smbios;
mod source;

use self::{
    disk::{first_disk_serial, proc_error_status},
    dmi::collect_dmi,
    policy::{
        ANCHOR_ORDER, EvidenceQuality, EvidenceStatus, MachineIdentityDecision, ReadOutcome,
        decide_machine_identity, evaluate_slot,
    },
    source::outcome_from_status,
};

/// Collects the three frozen hardware readings from a filesystem rooted at `filesystem_root`.
/// Production passes `/`; tests pass an isolated fixture root.
fn collect(filesystem_root: &Path) -> [ReadOutcome; 3] {
    let [system_uuid, board_serial] = collect_dmi(filesystem_root);
    let disk_serial = match Process::myself().and_then(|process| process.mountinfo()) {
        Ok(mounts) => first_disk_serial(filesystem_root, &mounts.0),
        Err(error) => outcome_from_status(proc_error_status(&error)),
    };
    [system_uuid, board_serial, disk_serial]
}

/// Runs collection and the frozen pure derivation pipeline without exposing source evidence.
///
/// # Errors
///
/// Returns the closed unavailable classification when the fixed sources cannot derive an ID.
pub(super) fn derive_identity(
    filesystem_root: &Path,
    fleet_namespace: Uuid,
) -> Result<DerivedMachineIdentity, MachineIdentityError> {
    identity_from_readings(collect(filesystem_root), fleet_namespace)
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

fn identity_from_readings(
    mut readings: [ReadOutcome; 3],
    fleet_namespace: Uuid,
) -> Result<DerivedMachineIdentity, MachineIdentityError> {
    let evaluations = std::array::from_fn(|index| {
        evaluate_slot(ANCHOR_ORDER[index], &readings[index], fleet_namespace)
    });
    let decision = decide_machine_identity(&evaluations);
    zeroize_readings(&mut readings);

    match decision {
        MachineIdentityDecision::Derived {
            machine_hardware_id,
            ..
        } => {
            let strong_sources = evaluations
                .iter()
                .filter(|evaluation| {
                    evaluation.status == EvidenceStatus::Present
                        && evaluation.quality == EvidenceQuality::Strong
                })
                .count();
            let quality = if strong_sources >= 2 {
                MachineIdentityQuality::Strong
            } else {
                MachineIdentityQuality::Medium
            };
            Ok(DerivedMachineIdentity {
                machine_hardware_id: machine_hardware_id.to_string(),
                quality,
            })
        }
        MachineIdentityDecision::InsufficientSources => {
            Err(MachineIdentityError::InsufficientSources(
                "machine identity requires at least two hardware sources".to_owned(),
            ))
        }
        MachineIdentityDecision::Unsupported => Err(MachineIdentityError::Unsupported(
            "machine identity is unsupported on this platform".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests;
