//! Compile-time ownership of Server-facing stable error codes.
//!
//! Runtime HTTP, WSS, Device Token authentication, and Enrollment-time certificate issuance are
//! outside the current persistence slice. These groups establish ownership without introducing
//! placeholder handlers.

use natsume_error_code::ErrorCode;

use crate::db::domain_checks::DomainCheckError;

/// Provisioning-window gate errors shared by every Enrollment issuance write.
pub const PROVISIONING_WINDOW_ERROR_CODES: &[ErrorCode] = &[ErrorCode::ProvisioningWindowClosed];

/// Provisioning-window Enrollment errors.
pub const ENROLLMENT_ERROR_CODES: &[ErrorCode] = &[ErrorCode::EnrollmentCsrInvalid];

/// Gateway certificate errors that remain possible during Enrollment issuance or installation.
pub const GATEWAY_CERT_ERROR_CODES: &[ErrorCode] = &[
    ErrorCode::GatewayCertIssuerUnavailable,
    ErrorCode::GatewayCertProfileInvalid,
    ErrorCode::GatewayCertLocalKeyMismatch,
    ErrorCode::GatewayCertInstallFailed,
];

/// Device Token-authenticated WSS control-plane protocol errors.
pub const CONTROL_PROTOCOL_ERROR_CODES: &[ErrorCode] = &[
    ErrorCode::ProtocolFrameTooLarge,
    ErrorCode::ProtocolInvalidEnvelope,
    ErrorCode::ProtocolAnonymousClient,
];

/// Server-owned Command request contract errors.
pub const COMMAND_ERROR_CODES: &[ErrorCode] = &[
    ErrorCode::CommandIdInvalid,
    ErrorCode::CommandRequestConflict,
];

/// Server vault errors.
pub const SERVER_VAULT_ERROR_CODES: &[ErrorCode] = &[ErrorCode::VaultCorrupt];

/// Server-owned groups in the Phase 0 minimum registry.
pub const SERVER_OWNED_ERROR_CODES: &[&[ErrorCode]] = &[
    PROVISIONING_WINDOW_ERROR_CODES,
    ENROLLMENT_ERROR_CODES,
    GATEWAY_CERT_ERROR_CODES,
    CONTROL_PROTOCOL_ERROR_CODES,
    COMMAND_ERROR_CODES,
    SERVER_VAULT_ERROR_CODES,
];

/// Maps persistence-policy failures that have a reviewed stable public meaning.
///
/// All other domain-check failures remain internal until a public boundary explicitly classifies
/// them. This preserves the dependency direction from typed domain errors to stable codes.
#[must_use]
pub const fn domain_check_error_code(error: &DomainCheckError) -> Option<ErrorCode> {
    match error {
        DomainCheckError::ProvisioningWindowClosed { .. } => {
            Some(ErrorCode::ProvisioningWindowClosed)
        }
        DomainCheckError::SiteIdentityAlreadyExists
        | DomainCheckError::SiteIdentityImmutable { .. }
        | DomainCheckError::SeatCodeRenameRequiresReplacement
        | DomainCheckError::MachineHardwareIdImmutable
        | DomainCheckError::EnrollmentRequestIdentityImmutable
        | DomainCheckError::IssuedEnrollmentImmutable
        | DomainCheckError::DeviceTokenImmutable
        | DomainCheckError::GatewayCertificateIdentityImmutable
        | DomainCheckError::GatewayCertificateStatusTransitionInvalid { .. }
        | DomainCheckError::ProvisioningWindowRevisionOverflow
        | DomainCheckError::ProvisioningWindowStateUnchanged
        | DomainCheckError::RecoveryWindowTransitionMustClose
        | DomainCheckError::PersistedProvisioningWindowInvalid
        | DomainCheckError::AuditBackingMissing
        | DomainCheckError::AuditBackingMismatch
        | DomainCheckError::ApprovedEnrollmentRequired
        | DomainCheckError::IssuedEnrollmentRequired
        | DomainCheckError::EnrollmentDeviceMismatch
        | DomainCheckError::EnrollmentMachineHardwareIdMismatch
        | DomainCheckError::EnrollmentDeviceNotEnrolled
        | DomainCheckError::EnrollmentResolutionMismatch { .. }
        | DomainCheckError::ReplacementCredentialsRequired
        | DomainCheckError::EnrollmentGatewaySpkiMismatch => None,
    }
}

#[cfg(test)]
mod tests {
    use natsume_error_code::{to_problem_details, to_protocol_code};
    use uuid::Uuid;

    use super::*;

    #[test]
    fn enrollment_and_gateway_certificate_namespaces_are_disjoint() {
        assert_eq!(
            PROVISIONING_WINDOW_ERROR_CODES,
            &[ErrorCode::ProvisioningWindowClosed]
        );
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
    fn command_errors_are_server_owned_with_stable_http_contracts() {
        assert_eq!(
            COMMAND_ERROR_CODES,
            &[
                ErrorCode::CommandIdInvalid,
                ErrorCode::CommandRequestConflict,
            ]
        );
        assert_eq!(ErrorCode::CommandIdInvalid.http_status(), 400);
        assert_eq!(ErrorCode::CommandRequestConflict.http_status(), 409);
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
