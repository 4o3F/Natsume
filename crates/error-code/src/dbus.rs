//! D-Bus error-name mapping without a zbus dependency.

use crate::ErrorCode;

/// Returns the well-known `org.natsume.Error.*` name for a stable code.
#[must_use]
pub const fn to_dbus_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InstallEndpointInvalidIp => "org.natsume.Error.InstallEndpointInvalidIp",
        ErrorCode::InstallEndpointInvalidPort => "org.natsume.Error.InstallEndpointInvalidPort",
        ErrorCode::ProtocolFrameTooLarge => "org.natsume.Error.ProtocolFrameTooLarge",
        ErrorCode::ProtocolInvalidEnvelope => "org.natsume.Error.ProtocolInvalidEnvelope",
        ErrorCode::ProtocolAnonymousClient => "org.natsume.Error.ProtocolAnonymousClient",
        ErrorCode::EnrollmentDeviceOnlyViolation => {
            "org.natsume.Error.EnrollmentDeviceOnlyViolation"
        }
        ErrorCode::EnrollmentCsrInvalid => "org.natsume.Error.EnrollmentCsrInvalid",
        ErrorCode::GatewayCertRequestNotAuthorized => {
            "org.natsume.Error.GatewayCertRequestNotAuthorized"
        }
        ErrorCode::GatewayCertCommandMismatch => "org.natsume.Error.GatewayCertCommandMismatch",
        ErrorCode::GatewayCertRequestExpired => "org.natsume.Error.GatewayCertRequestExpired",
        ErrorCode::GatewayCertCsrInvalid => "org.natsume.Error.GatewayCertCsrInvalid",
        ErrorCode::GatewayCertSpkiConflict => "org.natsume.Error.GatewayCertSpkiConflict",
        ErrorCode::GatewayCertIssuerUnavailable => "org.natsume.Error.GatewayCertIssuerUnavailable",
        ErrorCode::GatewayCertProfileInvalid => "org.natsume.Error.GatewayCertProfileInvalid",
        ErrorCode::GatewayCertLocalKeyMismatch => "org.natsume.Error.GatewayCertLocalKeyMismatch",
        ErrorCode::GatewayCertInstallFailed => "org.natsume.Error.GatewayCertInstallFailed",
        ErrorCode::VaultCorrupt => "org.natsume.Error.VaultCorrupt",
        ErrorCode::SessionChanged => "org.natsume.Error.SessionChanged",
        ErrorCode::StaleLockEpoch => "org.natsume.Error.StaleLockEpoch",
        ErrorCode::LockCommandMismatch => "org.natsume.Error.LockCommandMismatch",
        ErrorCode::NoActiveLock => "org.natsume.Error.NoActiveLock",
        ErrorCode::HomeTransition => "org.natsume.Error.HomeTransition",
        ErrorCode::PackageLayoutInvalid => "org.natsume.Error.PackageLayoutInvalid",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::ALL_ERROR_CODES;

    #[test]
    fn every_code_has_a_unique_dbus_name() {
        let mut names = BTreeSet::new();

        for code in ALL_ERROR_CODES {
            let name = to_dbus_name(code);
            assert!(name.starts_with("org.natsume.Error."));
            assert!(names.insert(name), "duplicate D-Bus name {name}");
        }
    }
}
