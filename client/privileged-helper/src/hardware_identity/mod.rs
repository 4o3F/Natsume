//! Linux hardware collection belongs to the privileged helper, not the shared crate.
//!
//! Collection order:
//! 1. `sysinfo::Product` and `sysinfo::Motherboard`.
//! 2. `smbios-lib` only for missing/chassis/processor fields and conflict checks.
//! 3. `raw-cpuid` only when a real Processor Serial leaf is present.
//! 4. `procfs::process::MountInfo` + `udev` for a uniquely resolved root disk.
//!
//! No app-local or `/etc/machine-id` fallback is accepted because it can be copied with the disk.
//!
//! Raw serials are normalized and hashed in this process, then zeroized/discarded.
//! No shell commands and no text parsing of dmidecode/lsblk/udevadm/findmnt are allowed.

use natsume_machine_identity::ReadOutcome;
use snafu::Snafu;

/// Collects the three frozen hardware identity readings inside the privileged helper.
///
/// # Errors
///
/// Returns [`HardwareCollectionError::NotImplemented`] until Probe D implements
/// the typed Linux collectors. This blueprint-only error intentionally has no stable
/// wire code.
pub fn collect() -> Result<[ReadOutcome; 3], HardwareCollectionError> {
    // Implementation deliberately omitted from the architecture blueprint.
    // Each source must return a typed status and fixture-testable evidence.
    Err(HardwareCollectionError::NotImplemented)
}

#[derive(Debug, Snafu)]
pub enum HardwareCollectionError {
    #[snafu(display("hardware collector is not implemented in the blueprint"))]
    NotImplemented,
}
