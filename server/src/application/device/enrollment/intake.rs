use uuid::Uuid;

#[cfg(test)]
use crate::application::device::credentials::NoLiveDeviceConnections;
use crate::{
    application::device::credentials::DeviceConnectionEvictor,
    audit::{AuditEventId, CorrelationId},
    db::{self, Database},
};

mod classify;
mod issue;

use self::{
    classify::{
        NewRequestPath, classify_new_request_path, current_credentials,
        device_facts_from_projection, issuance_device_context, live_request_facts_from_projection,
        require_hardware_not_rejected, require_open_window, same_digest,
        validate_current_credentials, validate_device_for_replacement, validate_replacement_device,
    },
    issue::{
        create_pending_request, issue_existing_request, issue_new_device,
        issue_same_spki_replacement,
    },
};
use super::{
    EnrollmentError, EnrollmentOutcome, EnrollmentRequestStatus, EnrollmentState, GatewayIssuer,
    IntakeIds, IssuanceReason, MAX_LIVE_ENROLLMENT_REQUESTS, PendingEnrollment,
    ValidatedEnrollmentRequest,
};

pub(super) async fn intake_validated<E>(
    database: &Database,
    issuer: GatewayIssuer,
    request: ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    connection_evictor: E,
) -> Result<EnrollmentOutcome, EnrollmentError>
where
    E: DeviceConnectionEvictor,
{
    intake_with_ids_and_connection_eviction(
        database,
        issuer,
        request,
        correlation_id,
        IntakeIds {
            request: Uuid::now_v7(),
            device: Uuid::now_v7(),
            certificate: Uuid::now_v7(),
            audit: AuditEventId::from_uuid(Uuid::now_v7()),
        },
        connection_evictor,
    )
    .await
}

#[cfg(test)]
async fn intake_with_ids(
    database: &Database,
    issuer: GatewayIssuer,
    request: ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    ids: IntakeIds,
) -> Result<EnrollmentOutcome, EnrollmentError> {
    intake_with_ids_and_connection_eviction(
        database,
        issuer,
        request,
        correlation_id,
        ids,
        NoLiveDeviceConnections,
    )
    .await
}

async fn intake_with_ids_and_connection_eviction<E>(
    database: &Database,
    issuer: GatewayIssuer,
    request: ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    ids: IntakeIds,
    connection_evictor: E,
) -> Result<EnrollmentOutcome, EnrollmentError>
where
    E: DeviceConnectionEvictor,
{
    // Nonblocking follow-up: measure and optimize this write-lock duration only
    // with a separately reviewed revalidation design; signing intentionally
    // remains inside the atomic guarded operation in this structural batch.
    database
        .write(move |transaction| {
            intake_in_transaction(
                transaction,
                &issuer,
                &request,
                correlation_id,
                ids,
                &connection_evictor,
            )
        })
        .await
}

fn intake_in_transaction<E>(
    transaction: &mut db::Transaction<'_>,
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    ids: IntakeIds,
    connection_evictor: &E,
) -> Result<EnrollmentOutcome, EnrollmentError>
where
    E: DeviceConnectionEvictor,
{
    require_open_window(transaction)?;
    let device =
        db::device::query::find_device_by_hardware(transaction, &request.machine_hardware_id)
            .map_err(EnrollmentError::from_device_persistence)?
            .map(device_facts_from_projection);

    require_hardware_not_rejected(transaction, &request.machine_hardware_id)?;

    let live = db::device::enrollment::query::live_requests_for_hardware(
        transaction,
        &request.machine_hardware_id,
    )?;
    if live.len() > 1 {
        return Err(EnrollmentError::InvalidPersistedFacts);
    }
    if let Some(live) = live.into_iter().next() {
        let live = live_request_facts_from_projection(live);
        if !same_digest(&live.gateway_spki_sha256, &request.gateway_spki_sha256) {
            return Err(EnrollmentError::DeviceIdentityConflict);
        }
        let device = validate_replacement_device(device.as_ref(), request, &live)?;
        return match live.state {
            EnrollmentRequestStatus::Pending => Ok(EnrollmentOutcome::Pending(PendingEnrollment {
                enrollment_request_id: live.enrollment_request_id,
                state: EnrollmentState::Pending,
            })),
            EnrollmentRequestStatus::Approved => {
                let has_current_credentials = current_credentials(
                    db::device::query::current_credential_consistency(
                        transaction,
                        &device.device_id,
                    )
                    .map_err(EnrollmentError::from_device_persistence)?,
                    &device.device_id,
                )?
                .is_some();
                let issuance_context =
                    issuance_device_context(device.state, has_current_credentials)?;
                issue_existing_request(
                    transaction,
                    issuer,
                    request,
                    correlation_id,
                    live.enrollment_request_id,
                    device.device_id,
                    ids.certificate,
                    ids.audit,
                    IssuanceReason::CredentialReplacement,
                    issuance_context,
                    connection_evictor,
                )
            }
            EnrollmentRequestStatus::Rejected
            | EnrollmentRequestStatus::Issued
            | EnrollmentRequestStatus::Expired
            | EnrollmentRequestStatus::Conflict => Err(EnrollmentError::InvalidPersistedFacts),
        };
    }

    let live_count = db::device::enrollment::query::live_request_count(transaction)?;
    if live_count >= MAX_LIVE_ENROLLMENT_REQUESTS {
        return Err(EnrollmentError::LiveRequestCapacityExceeded);
    }

    let current = if let Some(device) = device.as_ref() {
        validate_device_for_replacement(device, request)?;
        let current = current_credentials(
            db::device::query::current_credential_consistency(transaction, &device.device_id)
                .map_err(EnrollmentError::from_device_persistence)?,
            &device.device_id,
        )?;
        validate_current_credentials(device.state, current.as_ref())?;
        current
    } else {
        None
    };
    match classify_new_request_path(
        device.as_ref(),
        current.as_ref(),
        &request.gateway_spki_sha256,
    ) {
        NewRequestPath::CreateDevice => {
            issue_new_device(transaction, issuer, request, correlation_id, ids)
        }
        NewRequestPath::SameSpkiReplacement { device_id } => issue_same_spki_replacement(
            transaction,
            issuer,
            request,
            correlation_id,
            device_id,
            ids,
            connection_evictor,
        ),
        NewRequestPath::CredentialReplacement { device_id } => create_pending_request(
            transaction,
            request,
            correlation_id,
            device_id,
            ids.request,
            ids.audit,
        ),
    }
}

#[cfg(test)]
#[path = "intake/tests.rs"]
mod tests;
