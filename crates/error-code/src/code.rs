//! Stable [`ErrorCode`] values and public titles.

/// Phase 0 minimum stable error-code registry.
///
/// Stable strings are defined only by [`Self::as_str`]. They must never be derived from
/// `Debug`, `Display`, translated text, or a source error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    InstallEndpointInvalidIp,
    InstallEndpointInvalidPort,
    ProtocolFrameTooLarge,
    ProtocolInvalidEnvelope,
    ProtocolAnonymousClient,
    EnrollmentDeviceOnlyViolation,
    EnrollmentCsrInvalid,
    GatewayCertRequestNotAuthorized,
    GatewayCertCommandMismatch,
    GatewayCertRequestExpired,
    GatewayCertCsrInvalid,
    GatewayCertSpkiConflict,
    GatewayCertIssuerUnavailable,
    GatewayCertProfileInvalid,
    GatewayCertLocalKeyMismatch,
    GatewayCertInstallFailed,
    VaultCorrupt,
    SessionChanged,
    StaleLockEpoch,
    LockCommandMismatch,
    NoActiveLock,
    HomeTransition,
    PackageLayoutInvalid,
}

/// Every Phase 0 minimum stable error code exactly once.
pub const ALL_ERROR_CODES: [ErrorCode; 23] = [
    ErrorCode::InstallEndpointInvalidIp,
    ErrorCode::InstallEndpointInvalidPort,
    ErrorCode::ProtocolFrameTooLarge,
    ErrorCode::ProtocolInvalidEnvelope,
    ErrorCode::ProtocolAnonymousClient,
    ErrorCode::EnrollmentDeviceOnlyViolation,
    ErrorCode::EnrollmentCsrInvalid,
    ErrorCode::GatewayCertRequestNotAuthorized,
    ErrorCode::GatewayCertCommandMismatch,
    ErrorCode::GatewayCertRequestExpired,
    ErrorCode::GatewayCertCsrInvalid,
    ErrorCode::GatewayCertSpkiConflict,
    ErrorCode::GatewayCertIssuerUnavailable,
    ErrorCode::GatewayCertProfileInvalid,
    ErrorCode::GatewayCertLocalKeyMismatch,
    ErrorCode::GatewayCertInstallFailed,
    ErrorCode::VaultCorrupt,
    ErrorCode::SessionChanged,
    ErrorCode::StaleLockEpoch,
    ErrorCode::LockCommandMismatch,
    ErrorCode::NoActiveLock,
    ErrorCode::HomeTransition,
    ErrorCode::PackageLayoutInvalid,
];

impl ErrorCode {
    /// Returns the stable wire/API string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InstallEndpointInvalidIp => "INSTALL_ENDPOINT_INVALID_IP",
            Self::InstallEndpointInvalidPort => "INSTALL_ENDPOINT_INVALID_PORT",
            Self::ProtocolFrameTooLarge => "PROTOCOL_FRAME_TOO_LARGE",
            Self::ProtocolInvalidEnvelope => "PROTOCOL_INVALID_ENVELOPE",
            Self::ProtocolAnonymousClient => "PROTOCOL_ANONYMOUS_CLIENT",
            Self::EnrollmentDeviceOnlyViolation => "ENROLLMENT_DEVICE_ONLY_VIOLATION",
            Self::EnrollmentCsrInvalid => "ENROLLMENT_CSR_INVALID",
            Self::GatewayCertRequestNotAuthorized => "GATEWAY_CERT_REQUEST_NOT_AUTHORIZED",
            Self::GatewayCertCommandMismatch => "GATEWAY_CERT_COMMAND_MISMATCH",
            Self::GatewayCertRequestExpired => "GATEWAY_CERT_REQUEST_EXPIRED",
            Self::GatewayCertCsrInvalid => "GATEWAY_CERT_CSR_INVALID",
            Self::GatewayCertSpkiConflict => "GATEWAY_CERT_SPKI_CONFLICT",
            Self::GatewayCertIssuerUnavailable => "GATEWAY_CERT_ISSUER_UNAVAILABLE",
            Self::GatewayCertProfileInvalid => "GATEWAY_CERT_PROFILE_INVALID",
            Self::GatewayCertLocalKeyMismatch => "GATEWAY_CERT_LOCAL_KEY_MISMATCH",
            Self::GatewayCertInstallFailed => "GATEWAY_CERT_INSTALL_FAILED",
            Self::VaultCorrupt => "VAULT_CORRUPT",
            Self::SessionChanged => "SESSION_CHANGED",
            Self::StaleLockEpoch => "STALE_LOCK_EPOCH",
            Self::LockCommandMismatch => "LOCK_COMMAND_MISMATCH",
            Self::NoActiveLock => "NO_ACTIVE_LOCK",
            Self::HomeTransition => "HOME_TRANSITION",
            Self::PackageLayoutInvalid => "PACKAGE_LAYOUT_INVALID",
        }
    }

    /// Returns non-stable operator-facing title text.
    #[must_use]
    pub const fn public_title(self) -> &'static str {
        match self {
            Self::InstallEndpointInvalidIp => "Invalid install endpoint IP",
            Self::InstallEndpointInvalidPort => "Invalid install endpoint port",
            Self::ProtocolFrameTooLarge => "Protocol frame too large",
            Self::ProtocolInvalidEnvelope => "Invalid protocol envelope",
            Self::ProtocolAnonymousClient => "Anonymous control client rejected",
            Self::EnrollmentDeviceOnlyViolation => "Enrollment accepts Device Identity only",
            Self::EnrollmentCsrInvalid => "Enrollment CSR invalid",
            Self::GatewayCertRequestNotAuthorized => "Gateway certificate request not authorized",
            Self::GatewayCertCommandMismatch => "Gateway certificate command mismatch",
            Self::GatewayCertRequestExpired => "Gateway certificate request expired",
            Self::GatewayCertCsrInvalid => "Gateway certificate CSR invalid",
            Self::GatewayCertSpkiConflict => "Gateway certificate SPKI conflict",
            Self::GatewayCertIssuerUnavailable => "Gateway certificate issuer unavailable",
            Self::GatewayCertProfileInvalid => "Gateway certificate profile invalid",
            Self::GatewayCertLocalKeyMismatch => "Gateway certificate local key mismatch",
            Self::GatewayCertInstallFailed => "Gateway certificate install failed",
            Self::VaultCorrupt => "Vault corrupt",
            Self::SessionChanged => "Session changed",
            Self::StaleLockEpoch => "Stale lock epoch",
            Self::LockCommandMismatch => "Lock command mismatch",
            Self::NoActiveLock => "No active lock",
            Self::HomeTransition => "Home transition rejected",
            Self::PackageLayoutInvalid => "Package layout invalid",
        }
    }

    /// Returns the explicit HTTP status mapping.
    #[must_use]
    pub const fn http_status(self) -> u16 {
        crate::http::http_status(self)
    }

    /// Returns the explicit protocol stable-string mapping.
    #[must_use]
    pub const fn protocol_code(self) -> &'static str {
        crate::protocol::to_protocol_code(self)
    }

    /// Returns the explicit D-Bus error-name mapping.
    #[must_use]
    pub const fn dbus_name(self) -> &'static str {
        crate::dbus::to_dbus_name(self)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn variant_index(code: ErrorCode) -> usize {
        match code {
            ErrorCode::InstallEndpointInvalidIp => 0,
            ErrorCode::InstallEndpointInvalidPort => 1,
            ErrorCode::ProtocolFrameTooLarge => 2,
            ErrorCode::ProtocolInvalidEnvelope => 3,
            ErrorCode::ProtocolAnonymousClient => 4,
            ErrorCode::EnrollmentDeviceOnlyViolation => 5,
            ErrorCode::EnrollmentCsrInvalid => 6,
            ErrorCode::GatewayCertRequestNotAuthorized => 7,
            ErrorCode::GatewayCertCommandMismatch => 8,
            ErrorCode::GatewayCertRequestExpired => 9,
            ErrorCode::GatewayCertCsrInvalid => 10,
            ErrorCode::GatewayCertSpkiConflict => 11,
            ErrorCode::GatewayCertIssuerUnavailable => 12,
            ErrorCode::GatewayCertProfileInvalid => 13,
            ErrorCode::GatewayCertLocalKeyMismatch => 14,
            ErrorCode::GatewayCertInstallFailed => 15,
            ErrorCode::VaultCorrupt => 16,
            ErrorCode::SessionChanged => 17,
            ErrorCode::StaleLockEpoch => 18,
            ErrorCode::LockCommandMismatch => 19,
            ErrorCode::NoActiveLock => 20,
            ErrorCode::HomeTransition => 21,
            ErrorCode::PackageLayoutInvalid => 22,
        }
    }

    #[test]
    fn minimum_set_has_exactly_23_unique_stable_strings() {
        let mut stable_strings = BTreeSet::new();
        let mut variants = [false; ALL_ERROR_CODES.len()];

        for code in ALL_ERROR_CODES {
            let index = variant_index(code);
            assert!(!variants[index], "duplicate variant {}", code.as_str());
            variants[index] = true;
            assert!(
                stable_strings.insert(code.as_str()),
                "duplicate stable string {}",
                code.as_str()
            );
            assert!(!code.as_str().chars().any(char::is_lowercase));
            assert_ne!(format!("{code:?}"), code.as_str());
            assert!(!code.public_title().is_empty());
        }

        assert!(variants.into_iter().all(|present| present));
    }
}
