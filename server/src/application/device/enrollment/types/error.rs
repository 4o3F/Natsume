use snafu::Snafu;

use crate::{
    application::{device::DevicePersistenceError, provisioning::ProvisioningPersistenceError},
    audit::AuditPersistenceError,
};

use super::super::GatewayIssuerError;

/// Redacted persistence boundary shared by Enrollment-request adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
#[snafu(module)]
pub(crate) enum EnrollmentRequestPersistenceError {
    #[snafu(display("persisted Enrollment request facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("Enrollment request persistence failed"))]
    PersistenceFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum EnrollmentError {
    #[snafu(display("the Enrollment request ID is invalid"))]
    InvalidRequestId,
    #[snafu(display("the machine hardware ID is invalid"))]
    InvalidMachineHardwareId,
    #[snafu(display("the hardware identity quality is invalid"))]
    InvalidHardwareIdentityQuality,
    #[snafu(display("the client version is invalid"))]
    InvalidClientVersion,
    #[snafu(display("the Enrollment protocol version is unsupported"))]
    UnsupportedProtocolVersion,
    #[snafu(display("the claimed Gateway SPKI digest is invalid"))]
    InvalidSpki,
    #[snafu(display("the Gateway CSR encoding is invalid"))]
    InvalidCsrEncoding,
    #[snafu(display("the Gateway CSR is invalid"))]
    InvalidCsr,
    #[snafu(display("the claimed Gateway SPKI digest does not match the CSR"))]
    SpkiMismatch,
    #[snafu(display("the provisioning window is closed"))]
    ProvisioningWindowClosed,
    #[snafu(display("the Enrollment request was rejected"))]
    RequestRejected,
    #[snafu(display("the live Enrollment request capacity is exhausted"))]
    LiveRequestCapacityExceeded,
    #[snafu(display("the device identity conflicts with a live Enrollment request"))]
    DeviceIdentityConflict,
    #[snafu(display("the Enrollment request is not pending"))]
    RequestNotPending,
    #[snafu(display("the persisted Enrollment facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("Enrollment entropy is unavailable"))]
    EntropyUnavailable,
    #[snafu(display("the Gateway issuance policy no longer has sufficient validity"))]
    IssuancePolicyExpired,
    #[snafu(display("Gateway certificate signing failed"))]
    SigningFailed,
    #[snafu(display("Enrollment persistence failed"))]
    PersistenceFailed,
}

impl EnrollmentError {
    pub(in crate::application::device::enrollment) const fn from_request_persistence(
        error: EnrollmentRequestPersistenceError,
    ) -> Self {
        match error {
            EnrollmentRequestPersistenceError::InvalidPersistedFacts => Self::InvalidPersistedFacts,
            EnrollmentRequestPersistenceError::PersistenceFailed => Self::PersistenceFailed,
        }
    }

    pub(in crate::application::device::enrollment) const fn from_provisioning_persistence(
        error: ProvisioningPersistenceError,
    ) -> Self {
        match error {
            ProvisioningPersistenceError::InvalidPersistedFacts => Self::InvalidPersistedFacts,
            ProvisioningPersistenceError::PersistenceFailed => Self::PersistenceFailed,
        }
    }

    pub(in crate::application::device::enrollment) const fn from_audit_persistence(
        error: AuditPersistenceError,
    ) -> Self {
        match error {
            AuditPersistenceError::PersistenceFailed => Self::PersistenceFailed,
        }
    }

    pub(in crate::application::device::enrollment) const fn from_device_persistence(
        error: DevicePersistenceError,
    ) -> Self {
        match error {
            DevicePersistenceError::InvalidPersistedFacts => Self::InvalidPersistedFacts,
            DevicePersistenceError::PersistenceFailed => Self::PersistenceFailed,
        }
    }
}

impl From<GatewayIssuerError> for EnrollmentError {
    fn from(error: GatewayIssuerError) -> Self {
        match error {
            GatewayIssuerError::ValidityTooShort => Self::IssuancePolicyExpired,
            GatewayIssuerError::EntropyUnavailable => Self::EntropyUnavailable,
            GatewayIssuerError::MaterialUnreadable
            | GatewayIssuerError::InvalidMaterial
            | GatewayIssuerError::TrustRootUnreadable
            | GatewayIssuerError::InvalidTrustRoot
            | GatewayIssuerError::TrustRootMismatch
            | GatewayIssuerError::InvalidCsr
            | GatewayIssuerError::ClockInvalid
            | GatewayIssuerError::SigningFailed => Self::SigningFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        application::{device::DevicePersistenceError, provisioning::ProvisioningPersistenceError},
        audit::AuditPersistenceError,
    };

    use super::{EnrollmentError, EnrollmentRequestPersistenceError};

    #[test]
    fn persistence_mappings_cover_every_neutral_variant() {
        assert_eq!(
            EnrollmentError::from_request_persistence(
                EnrollmentRequestPersistenceError::InvalidPersistedFacts
            ),
            EnrollmentError::InvalidPersistedFacts
        );
        assert_eq!(
            EnrollmentError::from_request_persistence(
                EnrollmentRequestPersistenceError::PersistenceFailed
            ),
            EnrollmentError::PersistenceFailed
        );
        assert_eq!(
            EnrollmentError::from_provisioning_persistence(
                ProvisioningPersistenceError::InvalidPersistedFacts
            ),
            EnrollmentError::InvalidPersistedFacts
        );
        assert_eq!(
            EnrollmentError::from_provisioning_persistence(
                ProvisioningPersistenceError::PersistenceFailed
            ),
            EnrollmentError::PersistenceFailed
        );
        assert_eq!(
            EnrollmentError::from_audit_persistence(AuditPersistenceError::PersistenceFailed),
            EnrollmentError::PersistenceFailed
        );
        assert_eq!(
            EnrollmentError::from_device_persistence(DevicePersistenceError::InvalidPersistedFacts),
            EnrollmentError::InvalidPersistedFacts
        );
        assert_eq!(
            EnrollmentError::from_device_persistence(DevicePersistenceError::PersistenceFailed),
            EnrollmentError::PersistenceFailed
        );
    }
}
