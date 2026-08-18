use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::db::{self, Database};

use super::{
    DeviceError, DeviceId, DevicePersistenceError, DeviceState,
    enrollment::identifier::parse_canonical_uuid,
};

const DEVICE_TOKEN_BYTES: usize = 32;

pub(crate) trait DeviceConnectionEvictor: Send + Sync + 'static {
    fn evict_device_connection(&self, device_pk: &str) -> bool;
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(in crate::application::device) struct NoLiveDeviceConnections;

#[cfg(test)]
impl DeviceConnectionEvictor for NoLiveDeviceConnections {
    fn evict_device_connection(&self, _device_pk: &str) -> bool {
        false
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct DeviceToken([u8; DEVICE_TOKEN_BYTES]);

impl DeviceToken {
    pub(crate) fn generate() -> Option<Self> {
        let mut bytes = [0_u8; DEVICE_TOKEN_BYTES];
        getrandom::fill(&mut bytes).ok()?;
        Some(Self(bytes))
    }

    pub(crate) fn sha256(&self) -> [u8; 32] {
        Sha256::digest(self.0).into()
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; DEVICE_TOKEN_BYTES] {
        &self.0
    }
}

pub(crate) struct DeviceTokenAuthenticationFacts {
    pub(crate) device_pk: DeviceId,
    pub(crate) machine_hardware_id: Uuid,
    pub(crate) token_hash: [u8; 32],
}

impl DeviceTokenAuthenticationFacts {
    pub(crate) fn from_persisted(
        device_pk: &str,
        machine_hardware_id: &str,
        token_hash: Vec<u8>,
    ) -> Result<Self, DevicePersistenceError> {
        Ok(Self {
            device_pk: DeviceId::parse(device_pk)
                .ok_or(DevicePersistenceError::InvalidPersistedFacts)?,
            machine_hardware_id: parse_canonical_uuid(machine_hardware_id, 5)
                .map_err(|()| DevicePersistenceError::InvalidPersistedFacts)?,
            token_hash: token_hash
                .try_into()
                .map_err(|_| DevicePersistenceError::InvalidPersistedFacts)?,
        })
    }
}

/// Exact persisted evidence for the current Device Token, its issuing request,
/// and the active Gateway certificate. Business consistency is checked by the
/// Enrollment workflow after this single-statement projection is read.
#[derive(Clone, Copy)]
pub(crate) struct CurrentCredentialConsistencyProjection {
    pub(in crate::application::device) token_count: i64,
    pub(in crate::application::device) gateway_spki_sha256: Option<[u8; 32]>,
    pub(in crate::application::device) token_request_id: Option<Uuid>,
    pub(in crate::application::device) request_is_issued: Option<bool>,
    pub(in crate::application::device) request_resolved_device_id: Option<DeviceId>,
    pub(in crate::application::device) request_issuance_audit_event_id: Option<Uuid>,
    pub(in crate::application::device) active_certificate_count: i64,
    pub(in crate::application::device) active_certificate_request_id: Option<Uuid>,
    pub(in crate::application::device) active_certificate_spki_sha256: Option<[u8; 32]>,
}

impl CurrentCredentialConsistencyProjection {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_persisted(
        token_count: i64,
        gateway_spki_sha256: Option<Vec<u8>>,
        token_request_id: Option<&str>,
        request_state: Option<&str>,
        request_resolved_device_id: Option<&str>,
        request_issuance_audit_event_id: Option<&str>,
        active_certificate_count: i64,
        active_certificate_request_id: Option<&str>,
        active_certificate_spki_sha256: Option<Vec<u8>>,
    ) -> Result<Self, DevicePersistenceError> {
        let gateway_spki_sha256 = match gateway_spki_sha256 {
            Some(value) => Some(
                value
                    .try_into()
                    .map_err(|_| DevicePersistenceError::InvalidPersistedFacts)?,
            ),
            None => None,
        };
        let token_request_id = match token_request_id {
            Some(value) => Some(
                parse_canonical_uuid(value, 7)
                    .map_err(|()| DevicePersistenceError::InvalidPersistedFacts)?,
            ),
            None => None,
        };
        let request_resolved_device_id = match request_resolved_device_id {
            Some(value) => {
                Some(DeviceId::parse(value).ok_or(DevicePersistenceError::InvalidPersistedFacts)?)
            }
            None => None,
        };
        let request_issuance_audit_event_id = match request_issuance_audit_event_id {
            Some(value) => Some(
                parse_canonical_uuid(value, 7)
                    .map_err(|()| DevicePersistenceError::InvalidPersistedFacts)?,
            ),
            None => None,
        };
        let active_certificate_request_id = match active_certificate_request_id {
            Some(value) => Some(
                parse_canonical_uuid(value, 7)
                    .map_err(|()| DevicePersistenceError::InvalidPersistedFacts)?,
            ),
            None => None,
        };
        let active_certificate_spki_sha256 = match active_certificate_spki_sha256 {
            Some(value) => Some(
                value
                    .try_into()
                    .map_err(|_| DevicePersistenceError::InvalidPersistedFacts)?,
            ),
            None => None,
        };
        Ok(Self {
            token_count,
            gateway_spki_sha256,
            token_request_id,
            request_is_issued: request_state.map(|state| state == "issued"),
            request_resolved_device_id,
            request_issuance_audit_event_id,
            active_certificate_count,
            active_certificate_request_id,
            active_certificate_spki_sha256,
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct IssuanceDeviceContext {
    pub(in crate::application::device) previous_device_state: Option<DeviceState>,
    pub(in crate::application::device) retire_existing: bool,
    pub(in crate::application::device) restore_enrolled: bool,
}

impl IssuanceDeviceContext {
    pub(in crate::application::device) const fn new_device() -> Self {
        Self {
            previous_device_state: None,
            retire_existing: false,
            restore_enrolled: false,
        }
    }

    pub(in crate::application::device) const fn replacement(
        state: DeviceState,
        retire_existing: bool,
    ) -> Self {
        Self {
            previous_device_state: Some(state),
            retire_existing,
            restore_enrolled: !matches!(state, DeviceState::Enrolled),
        }
    }
}

pub(crate) async fn device_token_authentication_facts(
    database: &Database,
    token_hash: [u8; 32],
) -> Result<Option<DeviceTokenAuthenticationFacts>, DeviceError> {
    database
        .read(move |transaction| {
            db::device::query::device_token_authentication_facts(transaction, token_hash)
        })
        .await
        .map_err(DeviceError::from_persistence)
}
