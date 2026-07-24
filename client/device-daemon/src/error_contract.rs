//! Compile-time ownership of Device Daemon stable error codes.
//!
//! Runtime QUIC, Gateway certificate, vault and Session handlers arrive in later steps. These
//! groups freeze the Daemon side of shared contracts without introducing placeholder handlers.

use natsume_error_code::ErrorCode;

/// Local package endpoint validation errors.
pub const INSTALL_ENDPOINT_ERROR_CODES: &[ErrorCode] = &[
    ErrorCode::InstallEndpointInvalidIp,
    ErrorCode::InstallEndpointInvalidPort,
];

/// Mandatory-mTLS control protocol errors shared with Server.
pub const CONTROL_PROTOCOL_ERROR_CODES: &[ErrorCode] = &[
    ErrorCode::ProtocolFrameTooLarge,
    ErrorCode::ProtocolInvalidEnvelope,
    ErrorCode::ProtocolAnonymousClient,
];

/// Gateway certificate errors for authenticated QUIC plus active `SYNC_STATE` only.
pub const GATEWAY_CERT_ERROR_CODES: &[ErrorCode] = &[
    ErrorCode::GatewayCertRequestNotAuthorized,
    ErrorCode::GatewayCertCommandMismatch,
    ErrorCode::GatewayCertRequestExpired,
    ErrorCode::GatewayCertCsrInvalid,
    ErrorCode::GatewayCertSpkiConflict,
    ErrorCode::GatewayCertIssuerUnavailable,
    ErrorCode::GatewayCertProfileInvalid,
    ErrorCode::GatewayCertLocalKeyMismatch,
    ErrorCode::GatewayCertInstallFailed,
];

/// Local encrypted-vault errors shared with Server.
pub const DEVICE_VAULT_ERROR_CODES: &[ErrorCode] = &[ErrorCode::VaultCorrupt];

/// Session lifecycle and lock errors observed by the Device Daemon.
pub const SESSION_ERROR_CODES: &[ErrorCode] = &[
    ErrorCode::SessionChanged,
    ErrorCode::SessionAgentMissing,
    ErrorCode::StaleLockEpoch,
    ErrorCode::LockCommandMismatch,
    ErrorCode::NoActiveLock,
];

/// Device Daemon-owned groups in the Phase 0 minimum registry.
pub const DEVICE_DAEMON_OWNED_ERROR_CODES: &[&[ErrorCode]] = &[
    INSTALL_ENDPOINT_ERROR_CODES,
    CONTROL_PROTOCOL_ERROR_CODES,
    GATEWAY_CERT_ERROR_CODES,
    DEVICE_VAULT_ERROR_CODES,
    SESSION_ERROR_CODES,
];

#[cfg(test)]
mod tests {
    use natsume_error_code::{to_dbus_name, to_problem_details, to_protocol_code};
    use uuid::Uuid;

    use super::*;

    #[test]
    fn every_daemon_code_has_explicit_surface_mappings() {
        for group in DEVICE_DAEMON_OWNED_ERROR_CODES {
            for code in *group {
                let problem = to_problem_details(*code, Uuid::nil());

                assert_eq!(problem.code(), code.as_str());
                assert!(problem.detail().is_none());
                assert_eq!(to_protocol_code(*code), code.as_str());
                assert!(to_dbus_name(*code).starts_with("org.natsume.Error."));
            }
        }
    }

    #[test]
    fn gateway_codes_are_not_enrollment_codes() {
        for code in GATEWAY_CERT_ERROR_CODES {
            assert!(code.as_str().starts_with("GATEWAY_CERT_"));
            assert!(!code.as_str().starts_with("ENROLLMENT_"));
        }
    }

    #[test]
    fn daemon_owns_missing_session_agent_detection() {
        assert!(SESSION_ERROR_CODES.contains(&ErrorCode::SessionAgentMissing));
    }
}
