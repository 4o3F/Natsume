use subtle::ConstantTimeEq as _;
use uuid::Uuid;

use crate::{
    application::{
        device::{
            DeviceByHardwareProjection, DeviceId, DeviceState, HardwareIdentityQuality,
            credentials::{CurrentCredentialConsistencyProjection, IssuanceDeviceContext},
        },
        provisioning::ProvisioningWindowState,
    },
    db::{self},
};

use super::super::{
    EnrollmentError, EnrollmentRequestStatus, EnrollmentResolution,
    LiveEnrollmentRequestProjection, ValidatedEnrollmentRequest,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LatestRequestPath {
    Eligible,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NewRequestPath {
    CreateDevice,
    SameSpkiReplacement { device_id: DeviceId },
    CredentialReplacement { device_id: DeviceId },
}

pub(super) fn require_hardware_not_rejected(
    transaction: &mut db::Transaction<'_>,
    machine_hardware_id: &str,
) -> Result<(), EnrollmentError> {
    let latest = db::device::enrollment::query::latest_request_for_hardware(
        transaction,
        machine_hardware_id,
    )?;
    match classify_latest_request(latest.map(|facts| facts.state)) {
        LatestRequestPath::Eligible => Ok(()),
        LatestRequestPath::Rejected => Err(EnrollmentError::RequestRejected),
    }
}

const fn classify_latest_request(
    latest_state: Option<EnrollmentRequestStatus>,
) -> LatestRequestPath {
    if matches!(latest_state, Some(EnrollmentRequestStatus::Rejected)) {
        LatestRequestPath::Rejected
    } else {
        LatestRequestPath::Eligible
    }
}

pub(super) fn require_open_window(
    transaction: &mut db::Transaction<'_>,
) -> Result<(), EnrollmentError> {
    let window = db::provisioning::read_window(transaction)
        .map_err(EnrollmentError::from_provisioning_persistence)?;
    match window.state {
        ProvisioningWindowState::Open => Ok(()),
        ProvisioningWindowState::Closed => Err(EnrollmentError::ProvisioningWindowClosed),
    }
}

pub(super) struct ClassifiedDeviceFacts {
    pub(super) device_id: DeviceId,
    hardware_identity_quality: HardwareIdentityQuality,
    pub(super) state: DeviceState,
}

pub(super) fn device_facts_from_projection(
    projection: DeviceByHardwareProjection,
) -> ClassifiedDeviceFacts {
    ClassifiedDeviceFacts {
        device_id: projection.device_id,
        hardware_identity_quality: projection.hardware_identity_quality,
        state: projection.state,
    }
}

pub(super) struct LiveRequestFacts {
    pub(super) enrollment_request_id: Uuid,
    pub(super) gateway_spki_sha256: [u8; 32],
    pub(super) state: EnrollmentRequestStatus,
    resolution: Option<EnrollmentResolution>,
    resolved_device_id: Option<DeviceId>,
}

pub(super) fn live_request_facts_from_projection(
    projection: LiveEnrollmentRequestProjection,
) -> LiveRequestFacts {
    LiveRequestFacts {
        enrollment_request_id: projection.enrollment_request_id,
        gateway_spki_sha256: projection.gateway_spki_sha256,
        state: projection.state,
        resolution: projection.resolution,
        resolved_device_id: projection.resolved_device_id,
    }
}

pub(super) fn validate_replacement_device<'a>(
    device: Option<&'a ClassifiedDeviceFacts>,
    request: &ValidatedEnrollmentRequest,
    live: &LiveRequestFacts,
) -> Result<&'a ClassifiedDeviceFacts, EnrollmentError> {
    let device = device.ok_or(EnrollmentError::InvalidPersistedFacts)?;
    validate_device_for_replacement(device, request)?;
    if live.resolution != Some(EnrollmentResolution::ReplaceDeviceCredentials)
        || live.resolved_device_id != Some(device.device_id)
    {
        return Err(EnrollmentError::InvalidPersistedFacts);
    }
    Ok(device)
}

pub(super) fn validate_device_for_replacement(
    device: &ClassifiedDeviceFacts,
    request: &ValidatedEnrollmentRequest,
) -> Result<(), EnrollmentError> {
    if device.hardware_identity_quality != request.hardware_identity_quality {
        return Err(EnrollmentError::DeviceIdentityConflict);
    }
    if device.state != DeviceState::Enrolled {
        return Err(EnrollmentError::DeviceIdentityConflict);
    }
    Ok(())
}

pub(super) struct CurrentCredentials {
    pub(super) gateway_spki_sha256: [u8; 32],
}

pub(super) fn current_credentials(
    projection: CurrentCredentialConsistencyProjection,
    device_id: &DeviceId,
) -> Result<Option<CurrentCredentials>, EnrollmentError> {
    match (projection.token_count, projection.active_certificate_count) {
        (0, 0)
            if projection.gateway_spki_sha256.is_none()
                && projection.token_request_id.is_none()
                && projection.request_is_issued.is_none()
                && projection.request_resolved_device_id.is_none()
                && projection.request_issuance_audit_event_id.is_none()
                && projection.active_certificate_request_id.is_none()
                && projection.active_certificate_spki_sha256.is_none() =>
        {
            Ok(None)
        }
        (1, 1) => {
            let gateway_spki_sha256 = projection
                .gateway_spki_sha256
                .ok_or(EnrollmentError::InvalidPersistedFacts)?;
            let token_request_id = projection
                .token_request_id
                .ok_or(EnrollmentError::InvalidPersistedFacts)?;
            let active_certificate_request_id = projection
                .active_certificate_request_id
                .ok_or(EnrollmentError::InvalidPersistedFacts)?;
            projection
                .request_issuance_audit_event_id
                .ok_or(EnrollmentError::InvalidPersistedFacts)?;
            let active_certificate_spki_sha256 = projection
                .active_certificate_spki_sha256
                .ok_or(EnrollmentError::InvalidPersistedFacts)?;
            if active_certificate_request_id != token_request_id
                || projection.request_is_issued != Some(true)
                || projection.request_resolved_device_id != Some(*device_id)
                || !same_digest(&gateway_spki_sha256, &active_certificate_spki_sha256)
            {
                return Err(EnrollmentError::InvalidPersistedFacts);
            }
            Ok(Some(CurrentCredentials {
                gateway_spki_sha256,
            }))
        }
        _ => Err(EnrollmentError::InvalidPersistedFacts),
    }
}

pub(super) fn validate_current_credentials(
    device_state: DeviceState,
    current: Option<&CurrentCredentials>,
) -> Result<(), EnrollmentError> {
    match (device_state, current) {
        (DeviceState::Enrolled, Some(_)) => Ok(()),
        (DeviceState::Enrolled, None) => Err(EnrollmentError::InvalidPersistedFacts),
        (DeviceState::Disabled | DeviceState::Revoked, _) => {
            Err(EnrollmentError::DeviceIdentityConflict)
        }
    }
}

pub(super) fn issuance_device_context(
    state: DeviceState,
    current: Option<&CurrentCredentials>,
) -> Result<IssuanceDeviceContext, EnrollmentError> {
    match (state, current) {
        (DeviceState::Enrolled, Some(_)) => Ok(IssuanceDeviceContext::replacement()),
        (DeviceState::Enrolled, None) => Err(EnrollmentError::InvalidPersistedFacts),
        (DeviceState::Disabled | DeviceState::Revoked, _) => {
            Err(EnrollmentError::DeviceIdentityConflict)
        }
    }
}

pub(super) fn classify_new_request_path(
    device: Option<&ClassifiedDeviceFacts>,
    current: Option<&CurrentCredentials>,
    presented_spki: &[u8; 32],
) -> NewRequestPath {
    match device {
        None => NewRequestPath::CreateDevice,
        Some(device)
            if device.state == DeviceState::Enrolled
                && current.is_some_and(|current| {
                    same_digest(&current.gateway_spki_sha256, presented_spki)
                }) =>
        {
            NewRequestPath::SameSpkiReplacement {
                device_id: device.device_id,
            }
        }
        Some(device) => NewRequestPath::CredentialReplacement {
            device_id: device.device_id,
        },
    }
}

pub(super) fn same_digest(persisted: &[u8; 32], presented: &[u8; 32]) -> bool {
    bool::from(persisted.ct_eq(presented))
}

#[cfg(test)]
mod tests {
    use super::{
        ClassifiedDeviceFacts, CurrentCredentials, LatestRequestPath, NewRequestPath,
        classify_latest_request, classify_new_request_path, issuance_device_context,
        validate_current_credentials, validate_device_for_replacement,
    };
    use crate::application::device::{
        DeviceId, DeviceState, HardwareIdentityQuality,
        enrollment::{EnrollmentError, EnrollmentRequestStatus, ValidatedEnrollmentRequest},
    };

    const DEVICE_ID: &str = "01900000-0000-7000-8000-000000000001";

    #[test]
    fn typed_classification_selects_create_same_spki_replacement_and_rejected_paths() {
        let presented_spki = [7_u8; 32];
        let other_spki = [8_u8; 32];
        let device_id =
            DeviceId::parse(DEVICE_ID).unwrap_or_else(|| panic!("test Device ID must be valid"));
        let mut device = ClassifiedDeviceFacts {
            device_id,
            hardware_identity_quality: HardwareIdentityQuality::Strong,
            state: DeviceState::Enrolled,
        };
        let same = CurrentCredentials {
            gateway_spki_sha256: presented_spki,
        };
        let different = CurrentCredentials {
            gateway_spki_sha256: other_spki,
        };

        assert_eq!(
            classify_new_request_path(None, None, &presented_spki),
            NewRequestPath::CreateDevice
        );
        assert_eq!(
            classify_new_request_path(Some(&device), Some(&same), &presented_spki),
            NewRequestPath::SameSpkiReplacement { device_id }
        );
        assert_eq!(
            classify_new_request_path(Some(&device), Some(&different), &presented_spki),
            NewRequestPath::CredentialReplacement { device_id }
        );
        assert_eq!(
            classify_latest_request(Some(EnrollmentRequestStatus::Rejected)),
            LatestRequestPath::Rejected
        );

        let request = ValidatedEnrollmentRequest {
            machine_hardware_id: "550e8400-e29b-51d4-a716-446655440000".to_owned(),
            hardware_identity_quality: HardwareIdentityQuality::Strong,
            gateway_csr_der: Vec::new(),
            gateway_spki_sha256: presented_spki,
            client_version: "classification-test".to_owned(),
            protocol_version: 1,
            source_ip: "192.0.2.1".to_owned(),
        };
        for state in [DeviceState::Disabled, DeviceState::Revoked] {
            device.state = state;
            assert_eq!(
                validate_device_for_replacement(&device, &request),
                Err(EnrollmentError::DeviceIdentityConflict)
            );
            assert_eq!(
                validate_current_credentials(state, Some(&same)),
                Err(EnrollmentError::DeviceIdentityConflict)
            );
            assert!(matches!(
                issuance_device_context(state, Some(&same)),
                Err(EnrollmentError::DeviceIdentityConflict)
            ));
        }
        assert_eq!(
            validate_current_credentials(DeviceState::Enrolled, None),
            Err(EnrollmentError::InvalidPersistedFacts)
        );
        assert!(matches!(
            issuance_device_context(DeviceState::Enrolled, None),
            Err(EnrollmentError::InvalidPersistedFacts)
        ));
    }
}
