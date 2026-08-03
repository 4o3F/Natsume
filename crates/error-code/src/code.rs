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
    ProvisioningWindowClosed,
    EnrollmentCsrInvalid,
    GatewayCertIssuerUnavailable,
    GatewayCertProfileInvalid,
    GatewayCertLocalKeyMismatch,
    GatewayCertInstallFailed,
    VaultCorrupt,
    SessionChanged,
    SessionIneligible,
    SessionAmbiguous,
    SessionAgentDuplicate,
    SessionAutostartShadowed,
    SessionAgentMissing,
    SessionDisplayUnavailable,
    SessionDisplayLost,
    SessionUiPresentedUnfocused,
    SessionLockUnsupported,
    SessionUnlockUnsupported,
    StaleLockEpoch,
    LockCommandMismatch,
    NoActiveLock,
    HomeTransition,
    PackageLayoutInvalid,
    CommandIdInvalid,
    CommandRequestConflict,
}

/// Every Phase 0 minimum stable error code exactly once.
pub const ALL_ERROR_CODES: [ErrorCode; 30] = [
    ErrorCode::InstallEndpointInvalidIp,
    ErrorCode::InstallEndpointInvalidPort,
    ErrorCode::ProtocolFrameTooLarge,
    ErrorCode::ProtocolInvalidEnvelope,
    ErrorCode::ProtocolAnonymousClient,
    ErrorCode::ProvisioningWindowClosed,
    ErrorCode::EnrollmentCsrInvalid,
    ErrorCode::GatewayCertIssuerUnavailable,
    ErrorCode::GatewayCertProfileInvalid,
    ErrorCode::GatewayCertLocalKeyMismatch,
    ErrorCode::GatewayCertInstallFailed,
    ErrorCode::VaultCorrupt,
    ErrorCode::SessionChanged,
    ErrorCode::SessionIneligible,
    ErrorCode::SessionAmbiguous,
    ErrorCode::SessionAgentDuplicate,
    ErrorCode::SessionAutostartShadowed,
    ErrorCode::SessionAgentMissing,
    ErrorCode::SessionDisplayUnavailable,
    ErrorCode::SessionDisplayLost,
    ErrorCode::SessionUiPresentedUnfocused,
    ErrorCode::SessionLockUnsupported,
    ErrorCode::SessionUnlockUnsupported,
    ErrorCode::StaleLockEpoch,
    ErrorCode::LockCommandMismatch,
    ErrorCode::NoActiveLock,
    ErrorCode::HomeTransition,
    ErrorCode::PackageLayoutInvalid,
    ErrorCode::CommandIdInvalid,
    ErrorCode::CommandRequestConflict,
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
            Self::ProvisioningWindowClosed => "PROVISIONING_WINDOW_CLOSED",
            Self::EnrollmentCsrInvalid => "ENROLLMENT_CSR_INVALID",
            Self::GatewayCertIssuerUnavailable => "GATEWAY_CERT_ISSUER_UNAVAILABLE",
            Self::GatewayCertProfileInvalid => "GATEWAY_CERT_PROFILE_INVALID",
            Self::GatewayCertLocalKeyMismatch => "GATEWAY_CERT_LOCAL_KEY_MISMATCH",
            Self::GatewayCertInstallFailed => "GATEWAY_CERT_INSTALL_FAILED",
            Self::VaultCorrupt => "VAULT_CORRUPT",
            Self::SessionChanged => "SESSION_CHANGED",
            Self::SessionIneligible => "SESSION_INELIGIBLE",
            Self::SessionAmbiguous => "SESSION_AMBIGUOUS",
            Self::SessionAgentDuplicate => "SESSION_AGENT_DUPLICATE",
            Self::SessionAutostartShadowed => "SESSION_AUTOSTART_SHADOWED",
            Self::SessionAgentMissing => "SESSION_AGENT_MISSING",
            Self::SessionDisplayUnavailable => "SESSION_DISPLAY_UNAVAILABLE",
            Self::SessionDisplayLost => "SESSION_DISPLAY_LOST",
            Self::SessionUiPresentedUnfocused => "SESSION_UI_PRESENTED_UNFOCUSED",
            Self::SessionLockUnsupported => "SESSION_LOCK_UNSUPPORTED",
            Self::SessionUnlockUnsupported => "SESSION_UNLOCK_UNSUPPORTED",
            Self::StaleLockEpoch => "STALE_LOCK_EPOCH",
            Self::LockCommandMismatch => "LOCK_COMMAND_MISMATCH",
            Self::NoActiveLock => "NO_ACTIVE_LOCK",
            Self::HomeTransition => "HOME_TRANSITION",
            Self::PackageLayoutInvalid => "PACKAGE_LAYOUT_INVALID",
            Self::CommandIdInvalid => "COMMAND_ID_INVALID",
            Self::CommandRequestConflict => "COMMAND_REQUEST_CONFLICT",
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
            Self::ProvisioningWindowClosed => "Provisioning window closed",
            Self::EnrollmentCsrInvalid => "Enrollment Gateway CSR invalid",
            Self::GatewayCertIssuerUnavailable => {
                "Enrollment Gateway certificate issuer unavailable"
            }
            Self::GatewayCertProfileInvalid => "Configured Gateway certificate profile invalid",
            Self::GatewayCertLocalKeyMismatch => {
                "Enrolled Gateway certificate does not match the local key"
            }
            Self::GatewayCertInstallFailed => "Enrolled Gateway certificate install failed",
            Self::VaultCorrupt => "Vault corrupt",
            Self::SessionChanged => "Session changed",
            Self::SessionIneligible => "Graphical session is not eligible",
            Self::SessionAmbiguous => "Graphical session is ambiguous",
            Self::SessionAgentDuplicate => "Session Agent instance already active",
            Self::SessionAutostartShadowed => "Session Agent autostart entry is shadowed",
            Self::SessionAgentMissing => "Session Agent is missing",
            Self::SessionDisplayUnavailable => "Graphical display is unavailable",
            Self::SessionDisplayLost => "Graphical display was lost",
            Self::SessionUiPresentedUnfocused => "Session UI presented without focus",
            Self::SessionLockUnsupported => "Desktop lock is unsupported",
            Self::SessionUnlockUnsupported => "Desktop unlock is unsupported",
            Self::StaleLockEpoch => "Stale lock epoch",
            Self::LockCommandMismatch => "Lock command mismatch",
            Self::NoActiveLock => "No active lock",
            Self::HomeTransition => "Home transition rejected",
            Self::PackageLayoutInvalid => "Package layout invalid",
            Self::CommandIdInvalid => "Invalid Command ID",
            Self::CommandRequestConflict => "Command request conflicts with existing Command",
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
            ErrorCode::ProvisioningWindowClosed => 5,
            ErrorCode::EnrollmentCsrInvalid => 6,
            ErrorCode::GatewayCertIssuerUnavailable => 7,
            ErrorCode::GatewayCertProfileInvalid => 8,
            ErrorCode::GatewayCertLocalKeyMismatch => 9,
            ErrorCode::GatewayCertInstallFailed => 10,
            ErrorCode::VaultCorrupt => 11,
            ErrorCode::SessionChanged => 12,
            ErrorCode::SessionIneligible => 13,
            ErrorCode::SessionAmbiguous => 14,
            ErrorCode::SessionAgentDuplicate => 15,
            ErrorCode::SessionAutostartShadowed => 16,
            ErrorCode::SessionAgentMissing => 17,
            ErrorCode::SessionDisplayUnavailable => 18,
            ErrorCode::SessionDisplayLost => 19,
            ErrorCode::SessionUiPresentedUnfocused => 20,
            ErrorCode::SessionLockUnsupported => 21,
            ErrorCode::SessionUnlockUnsupported => 22,
            ErrorCode::StaleLockEpoch => 23,
            ErrorCode::LockCommandMismatch => 24,
            ErrorCode::NoActiveLock => 25,
            ErrorCode::HomeTransition => 26,
            ErrorCode::PackageLayoutInvalid => 27,
            ErrorCode::CommandIdInvalid => 28,
            ErrorCode::CommandRequestConflict => 29,
        }
    }

    #[test]
    fn minimum_set_has_exactly_30_unique_stable_strings() {
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
