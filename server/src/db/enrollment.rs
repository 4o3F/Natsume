use diesel::{
    OptionalExtension, QueryableByName, RunQueryDsl,
    sql_types::{Binary, Text},
};
use snafu::Snafu;

use crate::{
    application::enrollment::{
        DeviceTokenAuthenticationFacts, EnrollmentError, EnrollmentRequestSummary,
    },
    db::Database,
};

mod decision;
mod intake;
mod issuance;
mod row;

use self::row::EnrollmentRequestSummaryRow;
pub(crate) use self::{
    decision::{approve_request, reject_request},
    intake::intake,
};

#[cfg(test)]
use self::{
    intake::{IntakeIds, intake_with_ids},
    row::CountRow,
};

pub(crate) const MAX_LIVE_ENROLLMENT_REQUESTS: i64 = 600;

pub(crate) async fn list_requests(
    database: &Database,
) -> Result<Vec<EnrollmentRequestSummary>, EnrollmentError> {
    database
        .interact(|connection| {
            diesel::sql_query(
                "SELECT enrollment_request_id, machine_hardware_id, hardware_identity_quality, \
                 gateway_spki_sha256, client_version, protocol_version, state, resolution, \
                 resolved_device_pk AS resolved_device_id, created_at, source_ip \
                 FROM enrollment_requests WHERE state IN ('pending', 'approved') \
                 ORDER BY created_at, enrollment_request_id",
            )
            .load::<EnrollmentRequestSummaryRow>(connection)
            .map_err(|_| EnrollmentStoreError::RequestReadFailed)?
            .into_iter()
            .map(|row| {
                EnrollmentRequestSummary::from_persisted(row.into_persisted())
                    .map_err(|_| EnrollmentStoreError::InvalidPersistedFacts)
            })
            .collect::<Result<Vec<_>, EnrollmentStoreError>>()
        })
        .await
        .map_err(|_| EnrollmentStoreError::AcquireFailed)?
        .map_err(EnrollmentError::from)
}

pub(crate) async fn device_token_authentication_facts(
    database: &Database,
    token_hash: [u8; 32],
) -> Result<Option<DeviceTokenAuthenticationFacts>, EnrollmentError> {
    database
        .interact(move |connection| {
            diesel::sql_query(
                "SELECT dt.device_pk AS device_pk, d.machine_hardware_id AS machine_hardware_id, \
                 dt.token_hash AS token_hash FROM device_tokens dt JOIN devices d \
                 ON d.device_pk = dt.device_pk WHERE dt.token_hash = ?",
            )
            .bind::<Binary, _>(token_hash.as_slice())
            .get_result::<DeviceTokenAuthenticationRow>(connection)
            .optional()
            .map(|row| {
                row.map(|row| DeviceTokenAuthenticationFacts {
                    device_pk: row.device_pk,
                    machine_hardware_id: row.machine_hardware_id,
                    token_hash: row.token_hash,
                })
            })
            .map_err(|_| EnrollmentStoreError::CredentialReadFailed)
        })
        .await
        .map_err(|_| EnrollmentStoreError::AcquireFailed)?
        .map_err(EnrollmentError::from)
}

#[derive(QueryableByName)]
struct DeviceTokenAuthenticationRow {
    #[diesel(sql_type = Text)]
    device_pk: String,
    #[diesel(sql_type = Text)]
    machine_hardware_id: String,
    #[diesel(sql_type = Binary)]
    token_hash: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(super) enum EnrollmentStoreError {
    #[snafu(display("the database connection could not be acquired"))]
    AcquireFailed,
    #[snafu(display("the Enrollment transaction failed"))]
    TransactionFailed,
    #[snafu(display("the provisioning window could not be read"))]
    WindowReadFailed,
    #[snafu(display("the provisioning window is closed"))]
    ProvisioningWindowClosed,
    #[snafu(display("the Device could not be read"))]
    DeviceReadFailed,
    #[snafu(display("the Device could not be inserted"))]
    DeviceInsertFailed,
    #[snafu(display("the Device could not be reactivated"))]
    DeviceMutationFailed,
    #[snafu(display("the Enrollment request could not be read"))]
    RequestReadFailed,
    #[snafu(display("the Enrollment request could not be inserted"))]
    RequestInsertFailed,
    #[snafu(display("the Enrollment request could not be mutated"))]
    RequestMutationFailed,
    #[snafu(display("the current credentials could not be read"))]
    CredentialReadFailed,
    #[snafu(display("the Device Token could not be written"))]
    TokenWriteFailed,
    #[snafu(display("the old Gateway certificate could not be retired"))]
    CertificateMutationFailed,
    #[snafu(display("the Gateway certificate metadata could not be inserted"))]
    CertificateInsertFailed,
    #[snafu(display("the audit event could not be inserted"))]
    AuditInsertFailed,
    #[snafu(display("the Enrollment facts changed concurrently"))]
    CompareAndSwapConflict,
    #[snafu(display("the persisted Enrollment facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("the Enrollment request was rejected"))]
    RequestRejected,
    #[snafu(display("the live Enrollment request capacity is exhausted"))]
    LiveRequestCapacityExceeded,
    #[snafu(display("the Device identity conflicts with a live request"))]
    DeviceIdentityConflict,
    #[snafu(display("the Enrollment request is not pending"))]
    RequestNotPending,
    #[snafu(display("Enrollment entropy is unavailable"))]
    EntropyUnavailable,
    #[snafu(display("the Gateway issuance policy is expired"))]
    IssuancePolicyExpired,
    #[snafu(display("Gateway certificate signing failed"))]
    SigningFailed,
}

impl EnrollmentStoreError {
    pub(super) fn from_application(error: EnrollmentError) -> Self {
        match error {
            EnrollmentError::EntropyUnavailable => Self::EntropyUnavailable,
            EnrollmentError::IssuancePolicyExpired => Self::IssuancePolicyExpired,
            EnrollmentError::LiveRequestCapacityExceeded => Self::LiveRequestCapacityExceeded,
            EnrollmentError::SigningFailed
            | EnrollmentError::InvalidRequestId
            | EnrollmentError::InvalidMachineHardwareId
            | EnrollmentError::InvalidHardwareIdentityQuality
            | EnrollmentError::InvalidClientVersion
            | EnrollmentError::UnsupportedProtocolVersion
            | EnrollmentError::InvalidSpki
            | EnrollmentError::InvalidCsrEncoding
            | EnrollmentError::InvalidCsr
            | EnrollmentError::SpkiMismatch
            | EnrollmentError::ProvisioningWindowClosed
            | EnrollmentError::RequestRejected
            | EnrollmentError::DeviceIdentityConflict
            | EnrollmentError::RequestNotPending
            | EnrollmentError::InvalidPersistedFacts
            | EnrollmentError::PersistenceFailed => Self::SigningFailed,
        }
    }
}

impl From<diesel::result::Error> for EnrollmentStoreError {
    fn from(_source: diesel::result::Error) -> Self {
        Self::TransactionFailed
    }
}

impl From<EnrollmentStoreError> for EnrollmentError {
    fn from(error: EnrollmentStoreError) -> Self {
        match error {
            EnrollmentStoreError::ProvisioningWindowClosed => Self::ProvisioningWindowClosed,
            EnrollmentStoreError::RequestRejected => Self::RequestRejected,
            EnrollmentStoreError::LiveRequestCapacityExceeded => Self::LiveRequestCapacityExceeded,
            EnrollmentStoreError::DeviceIdentityConflict => Self::DeviceIdentityConflict,
            EnrollmentStoreError::RequestNotPending => Self::RequestNotPending,
            EnrollmentStoreError::InvalidPersistedFacts => Self::InvalidPersistedFacts,
            EnrollmentStoreError::EntropyUnavailable => Self::EntropyUnavailable,
            EnrollmentStoreError::IssuancePolicyExpired => Self::IssuancePolicyExpired,
            EnrollmentStoreError::SigningFailed => Self::SigningFailed,
            EnrollmentStoreError::AcquireFailed
            | EnrollmentStoreError::TransactionFailed
            | EnrollmentStoreError::WindowReadFailed
            | EnrollmentStoreError::DeviceReadFailed
            | EnrollmentStoreError::DeviceInsertFailed
            | EnrollmentStoreError::DeviceMutationFailed
            | EnrollmentStoreError::RequestReadFailed
            | EnrollmentStoreError::RequestInsertFailed
            | EnrollmentStoreError::RequestMutationFailed
            | EnrollmentStoreError::CredentialReadFailed
            | EnrollmentStoreError::TokenWriteFailed
            | EnrollmentStoreError::CertificateMutationFailed
            | EnrollmentStoreError::CertificateInsertFailed
            | EnrollmentStoreError::AuditInsertFailed
            | EnrollmentStoreError::CompareAndSwapConflict => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
mod tests;
