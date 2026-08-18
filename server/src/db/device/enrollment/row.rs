use diesel::{
    QueryableByName,
    sql_types::{BigInt, Binary, Nullable, Text},
};

use crate::application::device::enrollment::{
    EnrollmentDecisionProjection, EnrollmentError, EnrollmentRequestSummary,
    LatestEnrollmentRequestProjection, LiveEnrollmentRequestProjection,
};

#[derive(QueryableByName)]
pub(super) struct LatestEnrollmentRequestRow {
    #[diesel(sql_type = Text)]
    state: String,
}

impl LatestEnrollmentRequestRow {
    pub(super) fn into_projection(
        self,
    ) -> Result<LatestEnrollmentRequestProjection, EnrollmentError> {
        LatestEnrollmentRequestProjection::from_persisted(&self.state)
    }
}

#[derive(QueryableByName)]
pub(super) struct LiveEnrollmentRequestRow {
    #[diesel(sql_type = Text)]
    enrollment_request_id: String,
    #[diesel(sql_type = Binary)]
    gateway_spki_sha256: Vec<u8>,
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = Nullable<Text>)]
    resolution: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    resolved_device_id: Option<String>,
}

impl LiveEnrollmentRequestRow {
    pub(super) fn into_projection(
        self,
    ) -> Result<LiveEnrollmentRequestProjection, EnrollmentError> {
        LiveEnrollmentRequestProjection::from_persisted(
            &self.enrollment_request_id,
            self.gateway_spki_sha256,
            &self.state,
            self.resolution.as_deref(),
            self.resolved_device_id.as_deref(),
        )
    }
}

#[derive(QueryableByName)]
pub(super) struct EnrollmentDecisionRow {
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = Nullable<Text>)]
    resolution: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    resolved_device_id: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    issuance_audit_event_id: Option<String>,
}

impl EnrollmentDecisionRow {
    pub(super) fn into_projection(self) -> Result<EnrollmentDecisionProjection, EnrollmentError> {
        EnrollmentDecisionProjection::from_persisted(
            &self.state,
            self.resolution.as_deref(),
            self.resolved_device_id.as_deref(),
            self.issuance_audit_event_id.as_deref(),
        )
    }
}

#[derive(QueryableByName)]
pub(super) struct EnrollmentRequestSummaryRow {
    #[diesel(sql_type = Text)]
    enrollment_request_id: String,
    #[diesel(sql_type = Text)]
    machine_hardware_id: String,
    #[diesel(sql_type = Text)]
    hardware_identity_quality: String,
    #[diesel(sql_type = Binary)]
    gateway_spki_sha256: Vec<u8>,
    #[diesel(sql_type = Text)]
    client_version: String,
    #[diesel(sql_type = BigInt)]
    protocol_version: i64,
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = Nullable<Text>)]
    resolution: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    resolved_device_id: Option<String>,
    #[diesel(sql_type = Text)]
    created_at: String,
    #[diesel(sql_type = Text)]
    source_ip: String,
}

impl EnrollmentRequestSummaryRow {
    pub(super) fn into_facts(self) -> Result<EnrollmentRequestSummary, EnrollmentError> {
        EnrollmentRequestSummary::from_persisted(
            &self.enrollment_request_id,
            &self.machine_hardware_id,
            &self.hardware_identity_quality,
            self.gateway_spki_sha256,
            self.client_version,
            self.protocol_version,
            &self.state,
            self.resolution.as_deref(),
            self.resolved_device_id.as_deref(),
            self.created_at,
            &self.source_ip,
        )
    }
}

#[derive(QueryableByName)]
pub(super) struct CountRow {
    #[diesel(sql_type = BigInt)]
    pub(super) value: i64,
}

#[cfg(test)]
mod tests {
    use super::{
        EnrollmentDecisionRow, EnrollmentError, LatestEnrollmentRequestRow,
        LiveEnrollmentRequestRow,
    };

    const REQUEST_ID: &str = "01900000-0000-7000-8000-000000000001";

    #[test]
    fn invalid_persisted_enrollment_enum_fails_at_the_projection_boundary() {
        let result = LatestEnrollmentRequestRow {
            state: "unknown".to_owned(),
        }
        .into_projection();
        assert!(matches!(
            result,
            Err(EnrollmentError::InvalidPersistedFacts)
        ));
    }

    #[test]
    fn invalid_persisted_enrollment_uuid_fails_at_the_projection_boundary() {
        let result = LiveEnrollmentRequestRow {
            enrollment_request_id: "not-a-canonical-uuid".to_owned(),
            gateway_spki_sha256: vec![0_u8; 32],
            state: "pending".to_owned(),
            resolution: None,
            resolved_device_id: None,
        }
        .into_projection();
        assert!(matches!(
            result,
            Err(EnrollmentError::InvalidPersistedFacts)
        ));
    }

    #[test]
    fn invalid_persisted_enrollment_digest_fails_at_the_projection_boundary() {
        let result = LiveEnrollmentRequestRow {
            enrollment_request_id: REQUEST_ID.to_owned(),
            gateway_spki_sha256: vec![0_u8; 31],
            state: "pending".to_owned(),
            resolution: None,
            resolved_device_id: None,
        }
        .into_projection();
        assert!(matches!(
            result,
            Err(EnrollmentError::InvalidPersistedFacts)
        ));
    }

    #[test]
    fn non_rfc4122_persisted_resolved_device_id_fails_at_the_projection_boundary() {
        let result = EnrollmentDecisionRow {
            state: "approved".to_owned(),
            resolution: Some("replace_device_credentials".to_owned()),
            resolved_device_id: Some("01900000-0000-7000-0000-000000000001".to_owned()),
            issuance_audit_event_id: None,
        }
        .into_projection();
        assert!(matches!(
            result,
            Err(EnrollmentError::InvalidPersistedFacts)
        ));
    }
}
