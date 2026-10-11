use serde::{Deserialize, Serialize};
use zbus::zvariant::Type;

use crate::DerivedMachineIdentity;

/// Redacted policy status of one fixed hardware source; contains no source evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum HardwareSourceStatus {
    Present,
    Unavailable,
    Unsupported,
    PermissionDenied,
    Malformed,
    RejectedPlaceholder,
}

/// Source statuses from the same collection and evaluation as the identity decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct HardwareIdentitySources {
    pub dmi_system_uuid: HardwareSourceStatus,
    pub dmi_board_serial: HardwareSourceStatus,
    pub first_disk_serial: HardwareSourceStatus,
}

/// Closed outcome of the frozen hardware identity policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MachineIdentityOutcome {
    Derived,
    InsufficientSources,
    Unsupported,
}

/// One Helper-owned identity decision and its redacted source diagnostics.
/// Only `Derived` carries an identity; unsuccessful quorum is not a transport error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MachineIdentityReport {
    pub outcome: MachineIdentityOutcome,
    pub identity: Option<DerivedMachineIdentity>,
    pub sources: HardwareIdentitySources,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_identity_report_has_a_stable_dbus_signature() {
        assert_eq!(<HardwareSourceStatus as Type>::SIGNATURE, "u");
        assert_eq!(<HardwareIdentitySources as Type>::SIGNATURE, "(uuu)");
        assert_eq!(<MachineIdentityOutcome as Type>::SIGNATURE, "u");
        assert_eq!(<DerivedMachineIdentity as Type>::SIGNATURE, "(su)");
        assert_eq!(<MachineIdentityReport as Type>::SIGNATURE, "(ua(su)(uuu))");
    }
}
