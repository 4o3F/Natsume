use std::net::IpAddr;

use uuid::Uuid;

use crate::application::device::{
    DeviceId, HardwareIdentityQuality, enrollment::identifier::parse_canonical_uuid,
};

use super::{
    EnrollmentError, EnrollmentRequestStatus, EnrollmentResolution, EnrollmentReviewState,
};

#[derive(Clone, Copy)]
pub(crate) struct LiveEnrollmentRequestProjection {
    pub(in crate::application::device::enrollment) enrollment_request_id: Uuid,
    pub(in crate::application::device::enrollment) gateway_spki_sha256: [u8; 32],
    pub(in crate::application::device::enrollment) state: EnrollmentRequestStatus,
    pub(in crate::application::device::enrollment) resolution: Option<EnrollmentResolution>,
    pub(in crate::application::device::enrollment) resolved_device_id: Option<DeviceId>,
}

impl LiveEnrollmentRequestProjection {
    pub(crate) fn from_persisted(
        enrollment_request_id: &str,
        gateway_spki_sha256: Vec<u8>,
        state: &str,
        resolution: Option<&str>,
        resolved_device_id: Option<&str>,
    ) -> Result<Self, EnrollmentError> {
        Ok(Self {
            enrollment_request_id: persisted_uuid(enrollment_request_id, 7)?,
            gateway_spki_sha256: persisted_sha256(gateway_spki_sha256)?,
            state: EnrollmentRequestStatus::from_persisted(state)?,
            resolution: resolution
                .map(EnrollmentResolution::from_persisted)
                .transpose()?,
            resolved_device_id: persisted_optional_device_id(resolved_device_id)?,
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct LatestEnrollmentRequestProjection {
    pub(in crate::application::device::enrollment) state: EnrollmentRequestStatus,
}

impl LatestEnrollmentRequestProjection {
    pub(crate) fn from_persisted(state: &str) -> Result<Self, EnrollmentError> {
        Ok(Self {
            state: EnrollmentRequestStatus::from_persisted(state)?,
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EnrollmentDecisionProjection {
    pub(in crate::application::device::enrollment) state: EnrollmentRequestStatus,
    pub(in crate::application::device::enrollment) resolution: Option<EnrollmentResolution>,
    pub(in crate::application::device::enrollment) resolved_device_id: Option<DeviceId>,
    pub(in crate::application::device::enrollment) issuance_audit_event_id: Option<Uuid>,
}

impl EnrollmentDecisionProjection {
    pub(crate) fn from_persisted(
        state: &str,
        resolution: Option<&str>,
        resolved_device_id: Option<&str>,
        issuance_audit_event_id: Option<&str>,
    ) -> Result<Self, EnrollmentError> {
        Ok(Self {
            state: EnrollmentRequestStatus::from_persisted(state)?,
            resolution: resolution
                .map(EnrollmentResolution::from_persisted)
                .transpose()?,
            resolved_device_id: persisted_optional_device_id(resolved_device_id)?,
            issuance_audit_event_id: persisted_optional_uuid(issuance_audit_event_id, 7)?,
        })
    }
}

/// Redacted live Enrollment facts exposed to authenticated operators.
pub(crate) struct EnrollmentRequestSummary {
    pub(crate) enrollment_request_id: Uuid,
    pub(crate) machine_hardware_id: Uuid,
    pub(crate) hardware_identity_quality: HardwareIdentityQuality,
    pub(crate) gateway_spki_sha256: [u8; 32],
    pub(crate) client_version: String,
    pub(crate) protocol_version: u32,
    pub(crate) state: EnrollmentReviewState,
    pub(crate) resolution: Option<EnrollmentResolution>,
    pub(crate) resolved_device_id: Option<DeviceId>,
    pub(crate) created_at: String,
    pub(crate) source_ip: String,
}

impl EnrollmentRequestSummary {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_persisted(
        enrollment_request_id: &str,
        machine_hardware_id: &str,
        hardware_identity_quality: &str,
        gateway_spki_sha256: Vec<u8>,
        client_version: String,
        protocol_version: i64,
        state: &str,
        resolution: Option<&str>,
        resolved_device_id: Option<&str>,
        created_at: String,
        source_ip: &str,
    ) -> Result<Self, EnrollmentError> {
        let enrollment_request_id = persisted_uuid(enrollment_request_id, 7)?;
        let machine_hardware_id = persisted_uuid(machine_hardware_id, 5)?;
        let gateway_spki_sha256 = persisted_sha256(gateway_spki_sha256)?;
        if client_version.is_empty()
            || !client_version.bytes().all(|byte| byte.is_ascii_graphic())
            || created_at.is_empty()
        {
            return Err(EnrollmentError::InvalidPersistedFacts);
        }
        let source_ip = source_ip
            .parse::<IpAddr>()
            .map_err(|_| EnrollmentError::InvalidPersistedFacts)?
            .to_string();
        Ok(Self {
            enrollment_request_id,
            machine_hardware_id,
            hardware_identity_quality: HardwareIdentityQuality::parse(hardware_identity_quality)
                .ok_or(EnrollmentError::InvalidPersistedFacts)?,
            gateway_spki_sha256,
            client_version,
            protocol_version: u32::try_from(protocol_version)
                .map_err(|_| EnrollmentError::InvalidPersistedFacts)?,
            state: EnrollmentReviewState::from_persisted(state)?,
            resolution: resolution
                .map(EnrollmentResolution::from_persisted)
                .transpose()?,
            resolved_device_id: persisted_optional_device_id(resolved_device_id)?,
            created_at,
            source_ip,
        })
    }
}

fn persisted_uuid(value: &str, version: usize) -> Result<Uuid, EnrollmentError> {
    parse_canonical_uuid(value, version).map_err(|()| EnrollmentError::InvalidPersistedFacts)
}

fn persisted_optional_uuid(
    value: Option<&str>,
    version: usize,
) -> Result<Option<Uuid>, EnrollmentError> {
    value
        .map(|value| persisted_uuid(value, version))
        .transpose()
}

fn persisted_optional_device_id(value: Option<&str>) -> Result<Option<DeviceId>, EnrollmentError> {
    value
        .map(|value| DeviceId::parse(value).ok_or(EnrollmentError::InvalidPersistedFacts))
        .transpose()
}

fn persisted_sha256(value: Vec<u8>) -> Result<[u8; 32], EnrollmentError> {
    value
        .try_into()
        .map_err(|_| EnrollmentError::InvalidPersistedFacts)
}
