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

use std::path::Path;

use natsume_local_control_api::{HardwareCandidate, SanitizedHardwareClaim};
use natsume_machine_identity::{
    ANCHOR_ORDER, CollectionCompleteness, EvidenceQuality, EvidenceStatus, MachineIdentityDecision,
    ReadOutcome, decide_machine_identity, evaluate_slot,
};
use procfs::{process::MountInfo, process::Process};
use raw_cpuid::CpuId;
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
mod smbios;
mod source;

use self::{
    disk::{first_disk_serial, proc_error_status},
    dmi::collect_dmi,
    source::outcome_from_status,
};

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
mod tests;
