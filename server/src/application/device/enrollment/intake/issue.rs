use uuid::Uuid;

use crate::{
    application::device::{
        DeviceConnectionEvictor, DeviceId, DeviceState,
        credentials::{DeviceToken, IssuanceDeviceContext, NewGatewayCertificate},
    },
    audit::{AuditEvent, AuditEventId, CorrelationId, DeviceCredentialsIssuedAuditFacts},
    db::{self},
};

use super::super::{
    EnrollmentError, EnrollmentOutcome, EnrollmentRequestStatus, EnrollmentResolution,
    EnrollmentState, GatewayIssuer, IntakeIds, IssuanceReason, IssuedEnrollment,
    IssuedGatewayCertificate, IssuedRequestMode, PendingEnrollment, ValidatedEnrollmentRequest,
};

pub(super) fn create_pending_request(
    transaction: &mut db::Transaction<'_>,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    device_id: DeviceId,
    enrollment_request_id: Uuid,
    audit_event_id: AuditEventId,
) -> Result<EnrollmentOutcome, EnrollmentError> {
    let event = AuditEvent::enrollment_request_created(
        audit_event_id,
        correlation_id,
        enrollment_request_id,
        EnrollmentResolution::ReplaceDeviceCredentials,
        request.gateway_spki_sha256,
    );
    db::audit::insert(transaction, &event).map_err(EnrollmentError::from_audit_persistence)?;
    db::device::enrollment::request::insert(
        transaction,
        request,
        enrollment_request_id,
        EnrollmentRequestStatus::Pending,
        Some(EnrollmentResolution::ReplaceDeviceCredentials),
        Some(device_id.value()),
        None,
    )
    .map_err(EnrollmentError::from_request_persistence)?;
    Ok(EnrollmentOutcome::Pending(PendingEnrollment {
        enrollment_request_id,
        state: EnrollmentState::Pending,
    }))
}

pub(super) fn issue_new_device(
    transaction: &mut db::Transaction<'_>,
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    ids: IntakeIds,
) -> Result<EnrollmentOutcome, EnrollmentError> {
    let material = prepare_issuance(issuer, request)?;
    let device_id =
        DeviceId::from_uuid(ids.device).ok_or(EnrollmentError::InvalidPersistedFacts)?;
    db::device::devices::insert(
        transaction,
        &device_id,
        &request.machine_hardware_id,
        request.hardware_identity_quality,
    )
    .map_err(EnrollmentError::from_device_persistence)?;
    persist_issued_request_and_credentials(
        transaction,
        request,
        correlation_id,
        ids.request,
        ids.device,
        ids.certificate,
        ids.audit,
        EnrollmentResolution::CreateDevice,
        IssuanceReason::FirstEnrollment,
        material,
        IssuedRequestMode::Insert,
        IssuanceDeviceContext::new_device(),
        false,
    )
}

pub(super) fn issue_same_spki_replacement<E>(
    transaction: &mut db::Transaction<'_>,
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    device_id: DeviceId,
    ids: IntakeIds,
    connection_evictor: &E,
) -> Result<EnrollmentOutcome, EnrollmentError>
where
    E: DeviceConnectionEvictor,
{
    let material = prepare_issuance(issuer, request)?;
    let evicted_live_connection = connection_evictor.evict_device_connection(&device_id.as_text());
    persist_issued_request_and_credentials(
        transaction,
        request,
        correlation_id,
        ids.request,
        device_id.value(),
        ids.certificate,
        ids.audit,
        EnrollmentResolution::ReplaceDeviceCredentials,
        IssuanceReason::SameSpkiRetry,
        material,
        IssuedRequestMode::Insert,
        IssuanceDeviceContext::replacement(),
        evicted_live_connection,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn issue_existing_request<E>(
    transaction: &mut db::Transaction<'_>,
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    enrollment_request_id: Uuid,
    device_id: DeviceId,
    certificate_id: Uuid,
    audit_event_id: AuditEventId,
    reason: IssuanceReason,
    issuance_context: IssuanceDeviceContext,
    connection_evictor: &E,
) -> Result<EnrollmentOutcome, EnrollmentError>
where
    E: DeviceConnectionEvictor,
{
    let material = prepare_issuance(issuer, request)?;
    let evicted_live_connection = connection_evictor.evict_device_connection(&device_id.as_text());
    persist_issued_request_and_credentials(
        transaction,
        request,
        correlation_id,
        enrollment_request_id,
        device_id.value(),
        certificate_id,
        audit_event_id,
        EnrollmentResolution::ReplaceDeviceCredentials,
        reason,
        material,
        IssuedRequestMode::ClaimApproved,
        issuance_context,
        evicted_live_connection,
    )
}

struct PreparedIssuance {
    token: DeviceToken,
    token_hash: [u8; 32],
    certificate: IssuedGatewayCertificate,
}

fn prepare_issuance(
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
) -> Result<PreparedIssuance, EnrollmentError> {
    let token = DeviceToken::generate().ok_or(EnrollmentError::EntropyUnavailable)?;
    let token_hash = token.sha256();
    let certificate = issuer
        .issue_from_csr(&request.gateway_csr_der)
        .map_err(EnrollmentError::from)?;
    Ok(PreparedIssuance {
        token,
        token_hash,
        certificate,
    })
}

#[allow(clippy::too_many_arguments)]
fn persist_issued_request_and_credentials(
    transaction: &mut db::Transaction<'_>,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    enrollment_request_id: Uuid,
    device_id: Uuid,
    certificate_id: Uuid,
    audit_event_id: AuditEventId,
    resolution: EnrollmentResolution,
    reason: IssuanceReason,
    material: PreparedIssuance,
    request_mode: IssuedRequestMode,
    issuance_context: IssuanceDeviceContext,
    evicted_live_connection: bool,
) -> Result<EnrollmentOutcome, EnrollmentError> {
    let event = AuditEvent::device_credentials_issued(
        audit_event_id,
        correlation_id,
        enrollment_request_id,
        DeviceCredentialsIssuedAuditFacts {
            resolution,
            reason,
            certificate_serial: material.certificate.serial.clone(),
            gateway_spki_sha256: request.gateway_spki_sha256,
            previous_device_state: issuance_context
                .previous_device_state
                .map(DeviceState::as_persisted),
            evicted_live_connection,
        },
    );
    db::audit::insert(transaction, &event).map_err(EnrollmentError::from_audit_persistence)?;
    let audit_event_id_text = event.audit_event_id_text();
    match request_mode {
        IssuedRequestMode::Insert => {
            db::device::enrollment::request::insert(
                transaction,
                request,
                enrollment_request_id,
                EnrollmentRequestStatus::Issued,
                Some(resolution),
                Some(device_id),
                Some(&audit_event_id_text),
            )
            .map_err(EnrollmentError::from_request_persistence)?;
        }
        IssuedRequestMode::ClaimApproved => {
            db::device::enrollment::request::compare_and_swap_approved_to_issued(
                transaction,
                enrollment_request_id,
                resolution,
                device_id,
                &audit_event_id_text,
            )
            .map_err(EnrollmentError::from_request_persistence)?;
        }
    }

    if issuance_context.retire_existing {
        let retired = db::device::certificates::retire_active(transaction, device_id)
            .map_err(EnrollmentError::from_device_persistence)?;
        if retired != 1 {
            return Err(EnrollmentError::InvalidPersistedFacts);
        }
    }

    db::device::tokens::upsert(
        transaction,
        device_id,
        enrollment_request_id,
        material.token_hash,
    )
    .map_err(EnrollmentError::from_device_persistence)?;
    let certificate_facts = NewGatewayCertificate::new(
        material.certificate.serial.clone(),
        material.certificate.not_after.clone(),
        request.gateway_spki_sha256,
    );
    db::device::certificates::insert(
        transaction,
        certificate_id,
        device_id,
        enrollment_request_id,
        &certificate_facts,
    )
    .map_err(EnrollmentError::from_device_persistence)?;
    if db::device::certificates::active_count(transaction, device_id)
        .map_err(EnrollmentError::from_device_persistence)?
        != 1
    {
        return Err(EnrollmentError::InvalidPersistedFacts);
    }
    Ok(EnrollmentOutcome::Issued(IssuedEnrollment {
        enrollment_request_id,
        device_id,
        device_token: material.token,
        gateway_leaf_der: material.certificate.leaf_der,
        gateway_chain_der: material.certificate.chain_der,
    }))
}
