//! Compile-time ownership of Server-facing stable error codes.
//!
//! Runtime HTTP, QUIC, and certificate issuance are outside Step 3. These groups freeze
//! ownership without introducing handlers or allowing Gateway material in Enrollment.

use natsume_error_code::ErrorCode;

/// Device Identity-only Enrollment errors.
pub const ENROLLMENT_ERROR_CODES: &[ErrorCode] = &[
    ErrorCode::EnrollmentDeviceOnlyViolation,
    ErrorCode::EnrollmentCsrInvalid,
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

/// Mandatory-mTLS control-plane protocol errors.
pub const CONTROL_PROTOCOL_ERROR_CODES: &[ErrorCode] = &[
    ErrorCode::ProtocolFrameTooLarge,
    ErrorCode::ProtocolInvalidEnvelope,
    ErrorCode::ProtocolAnonymousClient,
];

/// Server vault errors.
pub const SERVER_VAULT_ERROR_CODES: &[ErrorCode] = &[ErrorCode::VaultCorrupt];

/// Server-owned groups in the Phase 0 minimum registry.
pub const SERVER_OWNED_ERROR_CODES: &[&[ErrorCode]] = &[
    ENROLLMENT_ERROR_CODES,
    GATEWAY_CERT_ERROR_CODES,
    CONTROL_PROTOCOL_ERROR_CODES,
    SERVER_VAULT_ERROR_CODES,
];

#[cfg(test)]
mod tests {
    use natsume_error_code::{to_problem_details, to_protocol_code};
    use uuid::Uuid;

    use super::*;

    #[test]
    fn enrollment_and_gateway_certificate_namespaces_are_disjoint() {
        for enrollment in ENROLLMENT_ERROR_CODES {
            assert!(enrollment.as_str().starts_with("ENROLLMENT_"));
            for gateway in GATEWAY_CERT_ERROR_CODES {
                assert_ne!(enrollment, gateway);
            }
        }

        for gateway in GATEWAY_CERT_ERROR_CODES {
            assert!(gateway.as_str().starts_with("GATEWAY_CERT_"));
        }
    }

    #[test]
    fn every_server_code_has_explicit_http_and_protocol_mappings() {
        for group in SERVER_OWNED_ERROR_CODES {
            for code in *group {
                let problem = to_problem_details(*code, Uuid::nil());
                assert_eq!(problem.code(), code.as_str());
                assert!(problem.detail().is_none());
                assert_eq!(to_protocol_code(*code), code.as_str());
            }
        }
    }
}
