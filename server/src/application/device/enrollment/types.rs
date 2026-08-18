use uuid::Uuid;

use crate::application::device::{HardwareIdentityQuality, credentials::DeviceToken};

mod error;
mod projections;

pub(crate) use self::error::EnrollmentError;
pub(crate) use self::projections::{
    EnrollmentDecisionProjection, EnrollmentRequestSummary, LatestEnrollmentRequestProjection,
    LiveEnrollmentRequestProjection,
};

use super::identifier::parse_canonical_uuid;

pub(crate) const ENROLLMENT_PROTOCOL_VERSION: u32 = 1;
pub(crate) const MAX_GATEWAY_CSR_DER_BYTES: usize = 32 * 1024;
pub(crate) const MAX_LIVE_ENROLLMENT_REQUESTS: i64 = 600;

#[derive(Clone)]
pub(crate) struct EnrollmentRequestInput {
    pub(crate) machine_hardware_id: String,
    pub(crate) hardware_identity_quality: HardwareIdentityQuality,
    pub(crate) gateway_csr_der: String,
    pub(crate) gateway_spki_sha256: String,
    pub(crate) client_version: String,
    pub(crate) protocol_version: u32,
}

pub(crate) struct ValidatedEnrollmentRequest {
    pub(crate) machine_hardware_id: String,
    pub(crate) hardware_identity_quality: HardwareIdentityQuality,
    pub(crate) gateway_csr_der: Vec<u8>,
    pub(crate) gateway_spki_sha256: [u8; 32],
    pub(crate) client_version: String,
    pub(crate) protocol_version: u32,
    pub(crate) source_ip: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentResolution {
    CreateDevice,
    ReplaceDeviceCredentials,
}

impl EnrollmentResolution {
    fn from_persisted(value: &str) -> Result<Self, EnrollmentError> {
        match value {
            "create_device" => Ok(Self::CreateDevice),
            "replace_device_credentials" => Ok(Self::ReplaceDeviceCredentials),
            _ => Err(EnrollmentError::InvalidPersistedFacts),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentReviewState {
    Pending,
    Approved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentRequestStatus {
    Pending,
    Approved,
    Rejected,
    Issued,
    Expired,
    Conflict,
}

impl EnrollmentRequestStatus {
    pub(in crate::application::device::enrollment) fn from_persisted(
        value: &str,
    ) -> Result<Self, EnrollmentError> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            "issued" => Ok(Self::Issued),
            "expired" => Ok(Self::Expired),
            "conflict" => Ok(Self::Conflict),
            _ => Err(EnrollmentError::InvalidPersistedFacts),
        }
    }

    pub(crate) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Issued => "issued",
            Self::Expired => "expired",
            Self::Conflict => "conflict",
        }
    }
}

impl EnrollmentReviewState {
    fn from_persisted(value: &str) -> Result<Self, EnrollmentError> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            _ => Err(EnrollmentError::InvalidPersistedFacts),
        }
    }
}

pub(crate) struct EnrollmentRequestId(Uuid);

impl EnrollmentRequestId {
    pub(crate) fn parse(value: &str) -> Result<Self, EnrollmentError> {
        let parsed =
            parse_canonical_uuid(value, 7).map_err(|()| EnrollmentError::InvalidRequestId)?;
        Ok(Self(parsed))
    }

    pub(crate) const fn value(&self) -> Uuid {
        self.0
    }

    #[cfg(test)]
    pub(crate) const fn for_test(value: Uuid) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentDecisionState {
    Approved,
    Rejected,
}

impl EnrollmentDecisionState {
    pub(crate) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

pub(crate) struct EnrollmentDecisionOutcome {
    pub(crate) enrollment_request_id: Uuid,
    pub(crate) state: EnrollmentDecisionState,
}

#[derive(Clone, Copy)]
pub(crate) enum IssuedRequestMode {
    Insert,
    ClaimApproved,
}

#[derive(Clone, Copy)]
pub(crate) struct IntakeIds {
    pub(crate) request: Uuid,
    pub(crate) device: Uuid,
    pub(crate) certificate: Uuid,
    pub(crate) audit: crate::audit::AuditEventId,
}

impl EnrollmentResolution {
    pub(crate) const fn as_persisted(self) -> &'static str {
        match self {
            Self::CreateDevice => "create_device",
            Self::ReplaceDeviceCredentials => "replace_device_credentials",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentState {
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IssuanceReason {
    FirstEnrollment,
    CredentialReplacement,
    SameSpkiRetry,
}

impl IssuanceReason {
    pub(crate) const fn as_audit_reason(self) -> &'static str {
        match self {
            Self::FirstEnrollment => "first_enrollment",
            Self::CredentialReplacement => "credential_replacement",
            Self::SameSpkiRetry => "same_spki_retry",
        }
    }
}

pub(crate) enum EnrollmentOutcome {
    Issued(IssuedEnrollment),
    Pending(PendingEnrollment),
}

pub(crate) struct IssuedEnrollment {
    pub(crate) enrollment_request_id: Uuid,
    pub(crate) device_id: Uuid,
    pub(crate) device_token: DeviceToken,
    pub(crate) gateway_leaf_der: Vec<u8>,
    pub(crate) gateway_chain_der: Vec<Vec<u8>>,
}

pub(crate) struct PendingEnrollment {
    pub(crate) enrollment_request_id: Uuid,
    pub(crate) state: EnrollmentState,
}
