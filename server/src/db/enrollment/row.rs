use diesel::{
    QueryableByName,
    sql_types::{BigInt, Binary, Nullable, Text},
};
use uuid::Uuid;

use crate::application::enrollment::PersistedEnrollmentRequestSummary;

use super::EnrollmentStoreError;

pub(super) fn canonical_uuid_v7(value: &str) -> Result<Uuid, EnrollmentStoreError> {
    let parsed = Uuid::parse_str(value).map_err(|_| EnrollmentStoreError::InvalidPersistedFacts)?;
    if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != value {
        return Err(EnrollmentStoreError::InvalidPersistedFacts);
    }
    Ok(parsed)
}

pub(super) struct DeviceRow {
    pub(super) device_id: Uuid,
    pub(super) hardware_identity_quality: String,
    pub(super) state: ReplacementDeviceState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReplacementDeviceState {
    Enrolled,
    Revoked,
    Disabled,
}

impl ReplacementDeviceState {
    pub(super) fn from_persisted(value: &str) -> Result<Self, EnrollmentStoreError> {
        match value {
            "enrolled" => Ok(Self::Enrolled),
            "revoked" => Ok(Self::Revoked),
            "disabled" => Ok(Self::Disabled),
            _ => Err(EnrollmentStoreError::InvalidPersistedFacts),
        }
    }

    pub(super) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Enrolled => "enrolled",
            Self::Revoked => "revoked",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(QueryableByName)]
pub(super) struct PersistedDeviceRow {
    #[diesel(sql_type = Text)]
    pub(super) device_pk: String,
    #[diesel(sql_type = Text)]
    pub(super) hardware_identity_quality: String,
    #[diesel(sql_type = Text)]
    pub(super) state: String,
}

#[derive(QueryableByName)]
pub(super) struct StateRow {
    #[diesel(sql_type = Text)]
    pub(super) state: String,
}

#[derive(QueryableByName)]
pub(super) struct LiveRequestRow {
    #[diesel(sql_type = Text)]
    pub(super) enrollment_request_id: String,
    #[diesel(sql_type = Binary)]
    pub(super) gateway_spki_sha256: Vec<u8>,
    #[diesel(sql_type = Text)]
    pub(super) state: String,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) resolution: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) resolved_device_pk: Option<String>,
}

pub(super) struct CurrentCredentialsRow {
    pub(super) gateway_spki_sha256: Vec<u8>,
}

#[derive(QueryableByName)]
pub(super) struct CurrentCredentialFactsRow {
    #[diesel(sql_type = BigInt)]
    pub(super) token_count: i64,
    #[diesel(sql_type = Nullable<Binary>)]
    pub(super) gateway_spki_sha256: Option<Vec<u8>>,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) token_request_id: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) request_state: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) request_resolved_device_pk: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) request_issuance_audit_event_id: Option<String>,
    #[diesel(sql_type = BigInt)]
    pub(super) active_certificate_count: i64,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) active_certificate_request_id: Option<String>,
    #[diesel(sql_type = Nullable<Binary>)]
    pub(super) active_certificate_spki_sha256: Option<Vec<u8>>,
}

#[derive(QueryableByName)]
pub(super) struct DecisionRequestRow {
    #[diesel(sql_type = Text)]
    pub(super) state: String,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) resolution: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) resolved_device_pk: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) issuance_audit_event_id: Option<String>,
}

#[derive(QueryableByName)]
pub(super) struct EnrollmentRequestSummaryRow {
    #[diesel(sql_type = Text)]
    pub(super) enrollment_request_id: String,
    #[diesel(sql_type = Text)]
    pub(super) machine_hardware_id: String,
    #[diesel(sql_type = Text)]
    pub(super) hardware_identity_quality: String,
    #[diesel(sql_type = Binary)]
    pub(super) gateway_spki_sha256: Vec<u8>,
    #[diesel(sql_type = Text)]
    pub(super) client_version: String,
    #[diesel(sql_type = BigInt)]
    pub(super) protocol_version: i64,
    #[diesel(sql_type = Text)]
    pub(super) state: String,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) resolution: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) resolved_device_id: Option<String>,
    #[diesel(sql_type = Text)]
    pub(super) created_at: String,
    #[diesel(sql_type = Text)]
    pub(super) source_ip: String,
}

impl EnrollmentRequestSummaryRow {
    pub(super) fn into_persisted(self) -> PersistedEnrollmentRequestSummary {
        PersistedEnrollmentRequestSummary {
            enrollment_request_id: self.enrollment_request_id,
            machine_hardware_id: self.machine_hardware_id,
            hardware_identity_quality: self.hardware_identity_quality,
            gateway_spki_sha256: self.gateway_spki_sha256,
            client_version: self.client_version,
            protocol_version: self.protocol_version,
            state: self.state,
            resolution: self.resolution,
            resolved_device_id: self.resolved_device_id,
            created_at: self.created_at,
            source_ip: self.source_ip,
        }
    }
}

#[derive(QueryableByName)]
pub(super) struct CountRow {
    #[diesel(sql_type = BigInt)]
    pub(super) value: i64,
}
