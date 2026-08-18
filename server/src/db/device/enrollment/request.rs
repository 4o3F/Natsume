use diesel::{
    RunQueryDsl,
    sql_types::{BigInt, Binary, Nullable, Text},
};
use uuid::Uuid;

use crate::{
    application::device::enrollment::{
        EnrollmentDecisionState, EnrollmentRequestPersistenceError, EnrollmentRequestStatus,
        EnrollmentResolution, ValidatedEnrollmentRequest,
    },
    db::Transaction,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    request: &ValidatedEnrollmentRequest,
    enrollment_request_id: Uuid,
    state: EnrollmentRequestStatus,
    resolution: Option<EnrollmentResolution>,
    resolved_device_id: Option<Uuid>,
    issuance_audit_event_id: Option<&str>,
) -> Result<(), EnrollmentRequestPersistenceError> {
    let inserted = diesel::sql_query(
        "INSERT INTO enrollment_requests (enrollment_request_id, machine_hardware_id, \
         hardware_identity_quality, gateway_csr_der, gateway_spki_sha256, client_version, \
         protocol_version, source_ip, state, resolution, resolved_device_pk, \
         issuance_audit_event_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
         strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
    )
    .bind::<Text, _>(enrollment_request_id.to_string())
    .bind::<Text, _>(&request.machine_hardware_id)
    .bind::<Text, _>(request.hardware_identity_quality.as_persisted())
    .bind::<Binary, _>(request.gateway_csr_der.as_slice())
    .bind::<Binary, _>(request.gateway_spki_sha256.as_slice())
    .bind::<Text, _>(&request.client_version)
    .bind::<BigInt, _>(i64::from(request.protocol_version))
    .bind::<Text, _>(&request.source_ip)
    .bind::<Text, _>(state.as_persisted())
    .bind::<Nullable<Text>, _>(resolution.map(EnrollmentResolution::as_persisted))
    .bind::<Nullable<Text>, _>(resolved_device_id.map(|id| id.to_string()))
    .bind::<Nullable<Text>, _>(issuance_audit_event_id)
    .execute(transaction.connection())
    .map_err(|_| EnrollmentRequestStoreError::InsertFailed)?;
    if inserted != 1 {
        return Err(EnrollmentRequestStoreError::AffectedRowCountInvalid.into());
    }
    Ok(())
}

pub(crate) fn compare_and_swap_approved_to_issued(
    transaction: &mut Transaction<'_>,
    enrollment_request_id: Uuid,
    resolution: EnrollmentResolution,
    resolved_device_id: Uuid,
    issuance_audit_event_id: &str,
) -> Result<(), EnrollmentRequestPersistenceError> {
    let updated = diesel::sql_query(
        "UPDATE enrollment_requests SET state = 'issued', resolution = ?, \
         resolved_device_pk = ?, issuance_audit_event_id = ? \
         WHERE enrollment_request_id = ? AND state = 'approved'",
    )
    .bind::<Text, _>(resolution.as_persisted())
    .bind::<Text, _>(resolved_device_id.to_string())
    .bind::<Text, _>(issuance_audit_event_id)
    .bind::<Text, _>(enrollment_request_id.to_string())
    .execute(transaction.connection())
    .map_err(|_| EnrollmentRequestStoreError::MutationFailed)?;
    if updated != 1 {
        return Err(EnrollmentRequestStoreError::CompareAndSwapConflict.into());
    }
    Ok(())
}

pub(crate) fn compare_and_swap_pending_to_decision(
    transaction: &mut Transaction<'_>,
    enrollment_request_id: Uuid,
    target_state: EnrollmentDecisionState,
) -> Result<(), EnrollmentRequestPersistenceError> {
    let updated = diesel::sql_query(
        "UPDATE enrollment_requests SET state = ? \
         WHERE enrollment_request_id = ? AND state = 'pending'",
    )
    .bind::<Text, _>(target_state.as_persisted())
    .bind::<Text, _>(enrollment_request_id.to_string())
    .execute(transaction.connection())
    .map_err(|_| EnrollmentRequestStoreError::MutationFailed)?;
    if updated != 1 {
        return Err(EnrollmentRequestStoreError::CompareAndSwapConflict.into());
    }
    Ok(())
}

pub(crate) fn expire_live_requests(
    transaction: &mut Transaction<'_>,
) -> Result<i64, EnrollmentRequestPersistenceError> {
    let expired = diesel::sql_query(
        "UPDATE enrollment_requests SET state = 'expired' \
         WHERE state IN ('pending', 'approved', 'rejected')",
    )
    .execute(transaction.connection())
    .map_err(|_| EnrollmentRequestStoreError::MutationFailed)?;
    i64::try_from(expired)
        .map_err(|_| EnrollmentRequestStoreError::AffectedRowCountInvalid)
        .map_err(EnrollmentRequestPersistenceError::from)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnrollmentRequestStoreError {
    InsertFailed,
    MutationFailed,
    CompareAndSwapConflict,
    AffectedRowCountInvalid,
}

impl From<EnrollmentRequestStoreError> for EnrollmentRequestPersistenceError {
    fn from(error: EnrollmentRequestStoreError) -> Self {
        match error {
            EnrollmentRequestStoreError::InsertFailed
            | EnrollmentRequestStoreError::MutationFailed
            | EnrollmentRequestStoreError::CompareAndSwapConflict
            | EnrollmentRequestStoreError::AffectedRowCountInvalid => Self::PersistenceFailed,
        }
    }
}
