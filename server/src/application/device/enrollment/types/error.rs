use snafu::Snafu;

use crate::application::device::DevicePersistenceError;

use super::super::GatewayIssuerError;

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
