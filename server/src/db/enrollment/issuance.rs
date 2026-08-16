use diesel::{
    RunQueryDsl,
    sql_types::{BigInt, Binary, Nullable, Text},
    sqlite::SqliteConnection,
};
use uuid::Uuid;

use crate::{
    application::enrollment::{
        DeviceToken, EnrollmentError, EnrollmentOutcome, EnrollmentResolution, GatewayIssuer,
        IssuanceReason, IssuedEnrollment, IssuedGatewayCertificate, ValidatedEnrollmentRequest,
    },
    audit::{self, AuditEvent, AuditEventId, CorrelationId, DeviceCredentialsIssuedAuditFacts},
};

use super::{
    EnrollmentStoreError,
    intake::IntakeIds,
    row::{CountRow, ReplacementDeviceState},
};

pub(super) fn issue_new_device(
    connection: &mut SqliteConnection,
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    ids: IntakeIds,
) -> Result<EnrollmentOutcome, EnrollmentStoreError> {
    let material = prepare_issuance(issuer, request)?;
    let device_id_text = ids.device.to_string();
    diesel::sql_query(
        "INSERT INTO devices (device_pk, machine_hardware_id, hardware_identity_quality, state) \
         VALUES (?, ?, ?, 'enrolled')",
    )
    .bind::<Text, _>(&device_id_text)
    .bind::<Text, _>(&request.machine_hardware_id)
    .bind::<Text, _>(request.hardware_identity_quality.as_persisted())
    .execute(connection)
    .map_err(|_| EnrollmentStoreError::DeviceInsertFailed)?;
    persist_issued_request_and_credentials(
        connection,
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
    )
}

pub(super) fn issue_same_spki_replacement(
    connection: &mut SqliteConnection,
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    device_id: Uuid,
    ids: IntakeIds,
) -> Result<EnrollmentOutcome, EnrollmentStoreError> {
    let material = prepare_issuance(issuer, request)?;
    persist_issued_request_and_credentials(
        connection,
        request,
        correlation_id,
        ids.request,
        device_id,
        ids.certificate,
        ids.audit,
        EnrollmentResolution::ReplaceDeviceCredentials,
        IssuanceReason::SameSpkiRetry,
        material,
        IssuedRequestMode::Insert,
        IssuanceDeviceContext::replacement(ReplacementDeviceState::Enrolled, true),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn issue_existing_request(
    connection: &mut SqliteConnection,
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    enrollment_request_id: Uuid,
    device_id: Uuid,
    certificate_id: Uuid,
    audit_event_id: AuditEventId,
    reason: IssuanceReason,
    issuance_context: IssuanceDeviceContext,
) -> Result<EnrollmentOutcome, EnrollmentStoreError> {
    let material = prepare_issuance(issuer, request)?;
    persist_issued_request_and_credentials(
        connection,
        request,
        correlation_id,
        enrollment_request_id,
        device_id,
        certificate_id,
        audit_event_id,
        EnrollmentResolution::ReplaceDeviceCredentials,
        reason,
        material,
        IssuedRequestMode::ClaimApproved,
        issuance_context,
    )
}

pub(super) struct PreparedIssuance {
    pub(super) token: DeviceToken,
    pub(super) token_hash: [u8; 32],
    pub(super) certificate: IssuedGatewayCertificate,
}

pub(super) fn prepare_issuance(
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
) -> Result<PreparedIssuance, EnrollmentStoreError> {
    let token = DeviceToken::generate().map_err(EnrollmentStoreError::from_application)?;
    let token_hash = token.sha256();
    let certificate = issuer
        .issue_from_csr(&request.gateway_csr_der)
        .map_err(EnrollmentError::from)
        .map_err(EnrollmentStoreError::from_application)?;
    Ok(PreparedIssuance {
        token,
        token_hash,
        certificate,
    })
}

#[derive(Clone, Copy)]
pub(super) enum IssuedRequestMode {
    Insert,
    ClaimApproved,
}

#[derive(Clone, Copy)]
pub(super) struct IssuanceDeviceContext {
    pub(super) previous_device_state: Option<&'static str>,
    pub(super) retire_existing: bool,
    pub(super) restore_enrolled: bool,
}

impl IssuanceDeviceContext {
    pub(super) const fn new_device() -> Self {
        Self {
            previous_device_state: None,
            retire_existing: false,
            restore_enrolled: false,
        }
    }

    pub(super) const fn replacement(state: ReplacementDeviceState, retire_existing: bool) -> Self {
        Self {
            previous_device_state: Some(state.as_persisted()),
            retire_existing,
            restore_enrolled: !matches!(state, ReplacementDeviceState::Enrolled),
        }
    }
}

pub(super) fn issuance_device_context(
    state: ReplacementDeviceState,
    has_current_credentials: bool,
) -> Result<IssuanceDeviceContext, EnrollmentStoreError> {
    match (state, has_current_credentials) {
        (ReplacementDeviceState::Enrolled | ReplacementDeviceState::Disabled, true)
        | (ReplacementDeviceState::Revoked, false) => Ok(IssuanceDeviceContext::replacement(
            state,
            has_current_credentials,
        )),
        (ReplacementDeviceState::Enrolled | ReplacementDeviceState::Disabled, false)
        | (ReplacementDeviceState::Revoked, true) => {
            Err(EnrollmentStoreError::InvalidPersistedFacts)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn persist_issued_request_and_credentials(
    connection: &mut SqliteConnection,
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
) -> Result<EnrollmentOutcome, EnrollmentStoreError> {
    let event = AuditEvent::device_credentials_issued(
        audit_event_id,
        correlation_id,
        enrollment_request_id,
        DeviceCredentialsIssuedAuditFacts {
            resolution,
            reason,
            certificate_serial: material.certificate.serial.clone(),
            gateway_spki_sha256: request.gateway_spki_sha256,
            previous_device_state: issuance_context.previous_device_state,
        },
    );
    audit::insert_diesel(connection, &event)
        .map_err(|_| EnrollmentStoreError::AuditInsertFailed)?;
    let audit_event_id_text = event.audit_event_id_text();
    match request_mode {
        IssuedRequestMode::Insert => insert_request(
            connection,
            request,
            enrollment_request_id,
            "issued",
            Some(resolution),
            Some(device_id),
            Some(&audit_event_id_text),
        )?,
        IssuedRequestMode::ClaimApproved => {
            let updated = diesel::sql_query(
                "UPDATE enrollment_requests SET state = 'issued', resolution = ?, \
                 resolved_device_pk = ?, issuance_audit_event_id = ? \
                 WHERE enrollment_request_id = ? AND state = 'approved'",
            )
            .bind::<Text, _>(resolution.as_persisted())
            .bind::<Text, _>(device_id.to_string())
            .bind::<Text, _>(&audit_event_id_text)
            .bind::<Text, _>(enrollment_request_id.to_string())
            .execute(connection)
            .map_err(|_| EnrollmentStoreError::RequestMutationFailed)?;
            if updated != 1 {
                return Err(EnrollmentStoreError::CompareAndSwapConflict);
            }
        }
    }

    apply_device_issuance_transition(connection, device_id, issuance_context)?;

    diesel::sql_query(
        "INSERT INTO device_tokens (device_pk, enrollment_request_id, token_hash) VALUES (?, ?, ?) \
         ON CONFLICT(device_pk) DO UPDATE SET enrollment_request_id = excluded.enrollment_request_id, \
         token_hash = excluded.token_hash",
    )
    .bind::<Text, _>(device_id.to_string())
    .bind::<Text, _>(enrollment_request_id.to_string())
    .bind::<Binary, _>(material.token_hash.as_slice())
    .execute(connection)
    .map_err(|_| EnrollmentStoreError::TokenWriteFailed)?;

    diesel::sql_query(
        "INSERT INTO gateway_certificates (certificate_id, device_pk, enrollment_request_id, \
         serial, spki_sha256, not_after, status) VALUES (?, ?, ?, ?, ?, ?, 'active')",
    )
    .bind::<Text, _>(certificate_id.to_string())
    .bind::<Text, _>(device_id.to_string())
    .bind::<Text, _>(enrollment_request_id.to_string())
    .bind::<Text, _>(&material.certificate.serial)
    .bind::<Binary, _>(request.gateway_spki_sha256.as_slice())
    .bind::<Text, _>(&material.certificate.not_after)
    .execute(connection)
    .map_err(|_| EnrollmentStoreError::CertificateInsertFailed)?;
    if active_certificate_count(connection, device_id)? != 1 {
        return Err(EnrollmentStoreError::InvalidPersistedFacts);
    }
    Ok(EnrollmentOutcome::Issued(IssuedEnrollment {
        enrollment_request_id,
        device_id,
        device_token: material.token,
        gateway_leaf_der: material.certificate.leaf_der,
        gateway_chain_der: material.certificate.chain_der,
    }))
}

pub(super) fn apply_device_issuance_transition(
    connection: &mut SqliteConnection,
    device_id: Uuid,
    context: IssuanceDeviceContext,
) -> Result<(), EnrollmentStoreError> {
    if context.restore_enrolled {
        let previous_device_state = context
            .previous_device_state
            .ok_or(EnrollmentStoreError::InvalidPersistedFacts)?;
        let updated = diesel::sql_query(
            "UPDATE devices SET state = 'enrolled' WHERE device_pk = ? AND state = ?",
        )
        .bind::<Text, _>(device_id.to_string())
        .bind::<Text, _>(previous_device_state)
        .execute(connection)
        .map_err(|_| EnrollmentStoreError::DeviceMutationFailed)?;
        if updated != 1 {
            return Err(EnrollmentStoreError::CompareAndSwapConflict);
        }
    }
    if context.retire_existing {
        let retired = diesel::sql_query(
            "UPDATE gateway_certificates SET status = 'retired' \
             WHERE device_pk = ? AND status = 'active'",
        )
        .bind::<Text, _>(device_id.to_string())
        .execute(connection)
        .map_err(|_| EnrollmentStoreError::CertificateMutationFailed)?;
        if retired != 1 {
            return Err(EnrollmentStoreError::InvalidPersistedFacts);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn insert_request(
    connection: &mut SqliteConnection,
    request: &ValidatedEnrollmentRequest,
    enrollment_request_id: Uuid,
    state: &str,
    resolution: Option<EnrollmentResolution>,
    resolved_device_id: Option<Uuid>,
    issuance_audit_event_id: Option<&str>,
) -> Result<(), EnrollmentStoreError> {
    diesel::sql_query(
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
    .bind::<Text, _>(state)
    .bind::<Nullable<Text>, _>(resolution.map(EnrollmentResolution::as_persisted))
    .bind::<Nullable<Text>, _>(resolved_device_id.map(|id| id.to_string()))
    .bind::<Nullable<Text>, _>(issuance_audit_event_id)
    .execute(connection)
    .map(|_| ())
    .map_err(|_| EnrollmentStoreError::RequestInsertFailed)
}

pub(super) fn active_certificate_count(
    connection: &mut SqliteConnection,
    device_id: Uuid,
) -> Result<i64, EnrollmentStoreError> {
    diesel::sql_query(
        "SELECT COUNT(*) AS value FROM gateway_certificates \
         WHERE device_pk = ? AND status = 'active'",
    )
    .bind::<Text, _>(device_id.to_string())
    .get_result::<CountRow>(connection)
    .map(|row| row.value)
    .map_err(|_| EnrollmentStoreError::CredentialReadFailed)
}
