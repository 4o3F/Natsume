use diesel::{
    OptionalExtension, QueryableByName, RunQueryDsl,
    sql_types::{BigInt, Binary, Nullable, Text},
    sqlite::SqliteConnection,
};
use snafu::Snafu;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    application::enrollment::{
        DeviceToken, EnrollmentError, EnrollmentOutcome, EnrollmentResolution, EnrollmentState,
        GatewayIssuer, IssuanceReason, IssuedEnrollment, IssuedGatewayCertificate,
        PendingEnrollment, ValidatedEnrollmentRequest,
    },
    audit::{self, AuditEvent, AuditEventId, CorrelationId},
    db::Database,
};

pub(crate) async fn intake(
    database: &Database,
    issuer: GatewayIssuer,
    request: ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
) -> Result<EnrollmentOutcome, EnrollmentError> {
    intake_with_ids(
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
    )
    .await
    .map_err(EnrollmentError::from)
}

#[derive(Clone, Copy)]
struct IntakeIds {
    request: Uuid,
    device: Uuid,
    certificate: Uuid,
    audit: AuditEventId,
}

async fn intake_with_ids(
    database: &Database,
    issuer: GatewayIssuer,
    request: ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    ids: IntakeIds,
) -> Result<EnrollmentOutcome, EnrollmentStoreError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                intake_in_transaction(connection, &issuer, &request, correlation_id, ids)
            })
        })
        .await
        .map_err(|_| EnrollmentStoreError::AcquireFailed)?
}

fn intake_in_transaction(
    connection: &mut SqliteConnection,
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    ids: IntakeIds,
) -> Result<EnrollmentOutcome, EnrollmentStoreError> {
    require_open_window(connection)?;
    let device = read_device(connection, &request.machine_hardware_id)?;
    let live = read_live_requests(connection, &request.machine_hardware_id)?;
    if live.len() > 1 {
        return Err(EnrollmentStoreError::InvalidPersistedFacts);
    }
    if let Some(live) = live.into_iter().next() {
        if !same_digest(&live.gateway_spki_sha256, &request.gateway_spki_sha256)? {
            return Err(EnrollmentStoreError::DeviceIdentityConflict);
        }
        let request_id = canonical_uuid_v7(&live.enrollment_request_id)?;
        let device = validate_replacement_device(device.as_ref(), request, &live)?;
        return match live.state.as_str() {
            "pending" => Ok(EnrollmentOutcome::Pending(PendingEnrollment {
                enrollment_request_id: request_id,
                state: EnrollmentState::Pending,
            })),
            "approved" => issue_existing_request(
                connection,
                issuer,
                request,
                correlation_id,
                request_id,
                device.device_id,
                ids.certificate,
                ids.audit,
                IssuanceReason::CredentialReplacement,
            ),
            _ => Err(EnrollmentStoreError::InvalidPersistedFacts),
        };
    }

    if latest_matching_state(connection, request)?.as_deref() == Some("rejected") {
        return Err(EnrollmentStoreError::RequestRejected);
    }

    let Some(device) = device else {
        return issue_new_device(connection, issuer, request, correlation_id, ids);
    };
    validate_device_for_replacement(&device, request)?;
    let current = read_current_credentials(connection, device.device_id)?;
    if let Some(current) = current
        && same_digest(&current.gateway_spki_sha256, &request.gateway_spki_sha256)?
    {
        return issue_new_replacement_request(
            connection,
            issuer,
            request,
            correlation_id,
            device.device_id,
            ids,
            IssuanceReason::SameSpkiRetry,
        );
    }
    create_pending_request(
        connection,
        request,
        correlation_id,
        device.device_id,
        ids.request,
        ids.audit,
    )
}

fn require_open_window(connection: &mut SqliteConnection) -> Result<(), EnrollmentStoreError> {
    let row = diesel::sql_query("SELECT state FROM provisioning_window WHERE singleton = 1")
        .get_result::<StateRow>(connection)
        .map_err(|_| EnrollmentStoreError::WindowReadFailed)?;
    match row.state.as_str() {
        "open" => Ok(()),
        "closed" => Err(EnrollmentStoreError::ProvisioningWindowClosed),
        _ => Err(EnrollmentStoreError::InvalidPersistedFacts),
    }
}

fn read_device(
    connection: &mut SqliteConnection,
    machine_hardware_id: &str,
) -> Result<Option<DeviceRow>, EnrollmentStoreError> {
    let row = diesel::sql_query(
        "SELECT device_pk, hardware_identity_quality, state FROM devices \
         WHERE machine_hardware_id = ?",
    )
    .bind::<Text, _>(machine_hardware_id)
    .get_result::<PersistedDeviceRow>(connection)
    .optional()
    .map_err(|_| EnrollmentStoreError::DeviceReadFailed)?;
    row.map(|row| {
        Ok(DeviceRow {
            device_id: canonical_uuid_v7(&row.device_pk)?,
            hardware_identity_quality: row.hardware_identity_quality,
            state: row.state,
        })
    })
    .transpose()
}

fn read_live_requests(
    connection: &mut SqliteConnection,
    machine_hardware_id: &str,
) -> Result<Vec<LiveRequestRow>, EnrollmentStoreError> {
    diesel::sql_query(
        "SELECT enrollment_request_id, gateway_spki_sha256, state, resolution, \
         resolved_device_pk FROM enrollment_requests WHERE machine_hardware_id = ? \
         AND state IN ('pending', 'approved') ORDER BY rowid",
    )
    .bind::<Text, _>(machine_hardware_id)
    .load(connection)
    .map_err(|_| EnrollmentStoreError::RequestReadFailed)
}

fn latest_matching_state(
    connection: &mut SqliteConnection,
    request: &ValidatedEnrollmentRequest,
) -> Result<Option<String>, EnrollmentStoreError> {
    diesel::sql_query(
        "SELECT state FROM enrollment_requests WHERE machine_hardware_id = ? \
         AND gateway_spki_sha256 = ? ORDER BY rowid DESC LIMIT 1",
    )
    .bind::<Text, _>(&request.machine_hardware_id)
    .bind::<Binary, _>(request.gateway_spki_sha256.as_slice())
    .get_result::<StateRow>(connection)
    .optional()
    .map(|row| row.map(|row| row.state))
    .map_err(|_| EnrollmentStoreError::RequestReadFailed)
}

fn validate_replacement_device<'a>(
    device: Option<&'a DeviceRow>,
    request: &ValidatedEnrollmentRequest,
    live: &LiveRequestRow,
) -> Result<&'a DeviceRow, EnrollmentStoreError> {
    let device = device.ok_or(EnrollmentStoreError::InvalidPersistedFacts)?;
    validate_device_for_replacement(device, request)?;
    let device_id = device.device_id.to_string();
    if live.resolution.as_deref() != Some("replace_device_credentials")
        || live.resolved_device_pk.as_deref() != Some(device_id.as_str())
    {
        return Err(EnrollmentStoreError::InvalidPersistedFacts);
    }
    Ok(device)
}

fn validate_device_for_replacement(
    device: &DeviceRow,
    request: &ValidatedEnrollmentRequest,
) -> Result<(), EnrollmentStoreError> {
    if device.state != "enrolled"
        || device.hardware_identity_quality != request.hardware_identity_quality.as_persisted()
    {
        return Err(EnrollmentStoreError::DeviceIdentityConflict);
    }
    Ok(())
}

fn read_current_credentials(
    connection: &mut SqliteConnection,
    device_id: Uuid,
) -> Result<Option<CurrentCredentialsRow>, EnrollmentStoreError> {
    let device_id = device_id.to_string();
    let row = diesel::sql_query(
        "SELECT (SELECT COUNT(*) FROM device_tokens dt WHERE dt.device_pk = ?) AS token_count, \
         (SELECT er.gateway_spki_sha256 FROM device_tokens dt JOIN enrollment_requests er \
          ON er.enrollment_request_id = dt.enrollment_request_id WHERE dt.device_pk = ? \
          LIMIT 1) AS gateway_spki_sha256, \
         (SELECT dt.enrollment_request_id FROM device_tokens dt WHERE dt.device_pk = ? \
          LIMIT 1) AS token_request_id, \
         (SELECT er.state FROM device_tokens dt JOIN enrollment_requests er \
          ON er.enrollment_request_id = dt.enrollment_request_id WHERE dt.device_pk = ? \
          LIMIT 1) AS request_state, \
         (SELECT er.resolved_device_pk FROM device_tokens dt JOIN enrollment_requests er \
          ON er.enrollment_request_id = dt.enrollment_request_id WHERE dt.device_pk = ? \
          LIMIT 1) AS request_resolved_device_pk, \
         (SELECT er.issuance_audit_event_id FROM device_tokens dt JOIN enrollment_requests er \
          ON er.enrollment_request_id = dt.enrollment_request_id WHERE dt.device_pk = ? \
          LIMIT 1) AS request_issuance_audit_event_id, \
         (SELECT COUNT(*) FROM gateway_certificates gc WHERE gc.device_pk = ? \
          AND gc.status = 'active') AS active_certificate_count, \
         (SELECT gc.enrollment_request_id FROM gateway_certificates gc WHERE gc.device_pk = ? \
          AND gc.status = 'active' LIMIT 1) AS active_certificate_request_id, \
         (SELECT gc.spki_sha256 FROM gateway_certificates gc WHERE gc.device_pk = ? \
          AND gc.status = 'active' LIMIT 1) AS active_certificate_spki_sha256",
    )
    .bind::<Text, _>(&device_id)
    .bind::<Text, _>(&device_id)
    .bind::<Text, _>(&device_id)
    .bind::<Text, _>(&device_id)
    .bind::<Text, _>(&device_id)
    .bind::<Text, _>(&device_id)
    .bind::<Text, _>(&device_id)
    .bind::<Text, _>(&device_id)
    .bind::<Text, _>(&device_id)
    .get_result::<CurrentCredentialFactsRow>(connection)
    .map_err(|_| EnrollmentStoreError::CredentialReadFailed)?;
    match (row.token_count, row.active_certificate_count) {
        (0, 0)
            if row.gateway_spki_sha256.is_none()
                && row.token_request_id.is_none()
                && row.request_state.is_none()
                && row.request_resolved_device_pk.is_none()
                && row.request_issuance_audit_event_id.is_none()
                && row.active_certificate_request_id.is_none()
                && row.active_certificate_spki_sha256.is_none() =>
        {
            Ok(None)
        }
        (1, 1) => {
            let gateway_spki_sha256 = row
                .gateway_spki_sha256
                .ok_or(EnrollmentStoreError::InvalidPersistedFacts)?;
            let token_request_id = row
                .token_request_id
                .ok_or(EnrollmentStoreError::InvalidPersistedFacts)?;
            canonical_uuid_v7(&token_request_id)?;
            let active_certificate_request_id = row
                .active_certificate_request_id
                .ok_or(EnrollmentStoreError::InvalidPersistedFacts)?;
            canonical_uuid_v7(&active_certificate_request_id)?;
            let issuance_audit_event_id = row
                .request_issuance_audit_event_id
                .ok_or(EnrollmentStoreError::InvalidPersistedFacts)?;
            canonical_uuid_v7(&issuance_audit_event_id)?;
            let active_certificate_spki_sha256 = row
                .active_certificate_spki_sha256
                .ok_or(EnrollmentStoreError::InvalidPersistedFacts)?;
            if gateway_spki_sha256.len() != 32
                || active_certificate_spki_sha256.len() != 32
                || active_certificate_request_id != token_request_id
                || row.request_state.as_deref() != Some("issued")
                || row.request_resolved_device_pk.as_deref() != Some(device_id.as_str())
                || !bool::from(gateway_spki_sha256.ct_eq(&active_certificate_spki_sha256))
            {
                return Err(EnrollmentStoreError::InvalidPersistedFacts);
            }
            Ok(Some(CurrentCredentialsRow {
                gateway_spki_sha256,
            }))
        }
        _ => Err(EnrollmentStoreError::InvalidPersistedFacts),
    }
}

fn same_digest(persisted: &[u8], presented: &[u8; 32]) -> Result<bool, EnrollmentStoreError> {
    if persisted.len() != 32 {
        return Err(EnrollmentStoreError::InvalidPersistedFacts);
    }
    Ok(bool::from(persisted.ct_eq(presented.as_slice())))
}

fn create_pending_request(
    connection: &mut SqliteConnection,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    device_id: Uuid,
    enrollment_request_id: Uuid,
    audit_event_id: AuditEventId,
) -> Result<EnrollmentOutcome, EnrollmentStoreError> {
    let event = AuditEvent::enrollment_request_created(
        audit_event_id,
        correlation_id,
        enrollment_request_id,
        EnrollmentResolution::ReplaceDeviceCredentials,
        request.gateway_spki_sha256,
    );
    audit::insert_diesel(connection, &event)
        .map_err(|_| EnrollmentStoreError::AuditInsertFailed)?;
    insert_request(
        connection,
        request,
        enrollment_request_id,
        "pending",
        Some(EnrollmentResolution::ReplaceDeviceCredentials),
        Some(device_id),
        None,
    )?;
    Ok(EnrollmentOutcome::Pending(PendingEnrollment {
        enrollment_request_id,
        state: EnrollmentState::Pending,
    }))
}

fn issue_new_device(
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
        false,
    )
}

fn issue_new_replacement_request(
    connection: &mut SqliteConnection,
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    device_id: Uuid,
    ids: IntakeIds,
    reason: IssuanceReason,
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
        reason,
        material,
        IssuedRequestMode::Insert,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn issue_existing_request(
    connection: &mut SqliteConnection,
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    enrollment_request_id: Uuid,
    device_id: Uuid,
    certificate_id: Uuid,
    audit_event_id: AuditEventId,
    reason: IssuanceReason,
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
        true,
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
enum IssuedRequestMode {
    Insert,
    ClaimApproved,
}

#[allow(clippy::too_many_arguments)]
fn persist_issued_request_and_credentials(
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
    retire_existing: bool,
) -> Result<EnrollmentOutcome, EnrollmentStoreError> {
    let event = AuditEvent::device_credentials_issued(
        audit_event_id,
        correlation_id,
        enrollment_request_id,
        resolution,
        reason,
        material.certificate.serial.clone(),
        request.gateway_spki_sha256,
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

    if retire_existing {
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

#[allow(clippy::too_many_arguments)]
fn insert_request(
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

fn active_certificate_count(
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

#[allow(dead_code)]
pub(crate) async fn approve_request(
    database: &Database,
    request_id: Uuid,
    correlation_id: CorrelationId,
) -> Result<(), EnrollmentError> {
    mutate_pending_request(
        database,
        request_id,
        correlation_id,
        EnrollmentDecision::Approve,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(EnrollmentError::from)
}

#[allow(dead_code)]
pub(crate) async fn reject_request(
    database: &Database,
    request_id: Uuid,
    correlation_id: CorrelationId,
) -> Result<(), EnrollmentError> {
    mutate_pending_request(
        database,
        request_id,
        correlation_id,
        EnrollmentDecision::Reject,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
    .map_err(EnrollmentError::from)
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
enum EnrollmentDecision {
    Approve,
    Reject,
}

#[allow(dead_code)]
async fn mutate_pending_request(
    database: &Database,
    request_id: Uuid,
    correlation_id: CorrelationId,
    decision: EnrollmentDecision,
    audit_event_id: AuditEventId,
) -> Result<(), EnrollmentStoreError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                let row = diesel::sql_query(
                    "SELECT state, resolution, resolved_device_pk, issuance_audit_event_id \
                     FROM enrollment_requests WHERE enrollment_request_id = ?",
                )
                .bind::<Text, _>(request_id.to_string())
                .get_result::<DecisionRequestRow>(connection)
                .optional()
                .map_err(|_| EnrollmentStoreError::RequestReadFailed)?
                .ok_or(EnrollmentStoreError::RequestNotFound)?;
                if row.state != "pending" {
                    return Err(EnrollmentStoreError::RequestNotPending);
                }
                if row.resolution.as_deref() != Some("replace_device_credentials")
                    || row.resolved_device_pk.is_none()
                    || row.issuance_audit_event_id.is_some()
                {
                    return Err(EnrollmentStoreError::InvalidPersistedFacts);
                }
                let (target_state, event) = match decision {
                    EnrollmentDecision::Approve => (
                        "approved",
                        AuditEvent::enrollment_request_approved(
                            audit_event_id,
                            correlation_id,
                            request_id,
                        ),
                    ),
                    EnrollmentDecision::Reject => (
                        "rejected",
                        AuditEvent::enrollment_request_rejected(
                            audit_event_id,
                            correlation_id,
                            request_id,
                        ),
                    ),
                };
                audit::insert_diesel(connection, &event)
                    .map_err(|_| EnrollmentStoreError::AuditInsertFailed)?;
                let updated = diesel::sql_query(
                    "UPDATE enrollment_requests SET state = ? \
                     WHERE enrollment_request_id = ? AND state = 'pending'",
                )
                .bind::<Text, _>(target_state)
                .bind::<Text, _>(request_id.to_string())
                .execute(connection)
                .map_err(|_| EnrollmentStoreError::RequestMutationFailed)?;
                if updated != 1 {
                    return Err(EnrollmentStoreError::CompareAndSwapConflict);
                }
                Ok(())
            })
        })
        .await
        .map_err(|_| EnrollmentStoreError::AcquireFailed)?
}

fn canonical_uuid_v7(value: &str) -> Result<Uuid, EnrollmentStoreError> {
    let parsed = Uuid::parse_str(value).map_err(|_| EnrollmentStoreError::InvalidPersistedFacts)?;
    if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != value {
        return Err(EnrollmentStoreError::InvalidPersistedFacts);
    }
    Ok(parsed)
}

struct DeviceRow {
    device_id: Uuid,
    hardware_identity_quality: String,
    state: String,
}

#[derive(QueryableByName)]
struct PersistedDeviceRow {
    #[diesel(sql_type = Text)]
    device_pk: String,
    #[diesel(sql_type = Text)]
    hardware_identity_quality: String,
    #[diesel(sql_type = Text)]
    state: String,
}

#[derive(QueryableByName)]
struct StateRow {
    #[diesel(sql_type = Text)]
    state: String,
}

#[derive(QueryableByName)]
struct LiveRequestRow {
    #[diesel(sql_type = Text)]
    enrollment_request_id: String,
    #[diesel(sql_type = Binary)]
    gateway_spki_sha256: Vec<u8>,
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = Nullable<Text>)]
    resolution: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    resolved_device_pk: Option<String>,
}

struct CurrentCredentialsRow {
    gateway_spki_sha256: Vec<u8>,
}

#[derive(QueryableByName)]
struct CurrentCredentialFactsRow {
    #[diesel(sql_type = BigInt)]
    token_count: i64,
    #[diesel(sql_type = Nullable<Binary>)]
    gateway_spki_sha256: Option<Vec<u8>>,
    #[diesel(sql_type = Nullable<Text>)]
    token_request_id: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    request_state: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    request_resolved_device_pk: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    request_issuance_audit_event_id: Option<String>,
    #[diesel(sql_type = BigInt)]
    active_certificate_count: i64,
    #[diesel(sql_type = Nullable<Text>)]
    active_certificate_request_id: Option<String>,
    #[diesel(sql_type = Nullable<Binary>)]
    active_certificate_spki_sha256: Option<Vec<u8>>,
}

#[allow(dead_code)]
#[derive(QueryableByName)]
struct DecisionRequestRow {
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = Nullable<Text>)]
    resolution: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    resolved_device_pk: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    issuance_audit_event_id: Option<String>,
}

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
enum EnrollmentStoreError {
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
    #[snafu(display("the Device identity conflicts with a live request"))]
    DeviceIdentityConflict,
    #[snafu(display("the Enrollment request does not exist"))]
    RequestNotFound,
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
    fn from_application(error: EnrollmentError) -> Self {
        match error {
            EnrollmentError::EntropyUnavailable => Self::EntropyUnavailable,
            EnrollmentError::IssuancePolicyExpired => Self::IssuancePolicyExpired,
            EnrollmentError::SigningFailed
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
            | EnrollmentError::RequestNotFound
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
            EnrollmentStoreError::DeviceIdentityConflict => Self::DeviceIdentityConflict,
            EnrollmentStoreError::RequestNotFound => Self::RequestNotFound,
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
mod tests {
    use std::{
        fs,
        net::{IpAddr, Ipv4Addr},
        path::PathBuf,
    };

    use diesel::{
        QueryableByName, RunQueryDsl,
        sql_types::{BigInt, Binary, Text},
    };
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PublicKeyData};
    use rustls::{client::verify_server_name, server::ParsedCertificate};
    use rustls_pki_types::{CertificateDer, ServerName};
    use sha2::{Digest, Sha256};
    use snafu::Snafu;
    use uuid::Uuid;

    use crate::{
        application::{
            enrollment::{
                self, EnrollmentError, EnrollmentOutcome, EnrollmentRequestInput, GatewayIssuer,
                HardwareIdentityQuality, TEST_GATEWAY_HOSTNAME, TEST_GATEWAY_NOT_AFTER,
                ValidatedEnrollmentRequest, encode_standard_base64,
            },
            provisioning,
        },
        audit::{AuditEventId, CorrelationId},
        config::GatewaySiteConfig,
        db::{Database, DatabaseConfig},
    };
    use x509_parser::{extensions::GeneralName, parse_x509_certificate};

    use super::{EnrollmentStoreError, IntakeIds, intake_with_ids};

    const SOURCE_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 44));
    const HOSTILE_CSR_SERIAL_SUGGESTION: [u8; 20] = [0x5a; 20];
    const EXPECTED_SUBJECT_COMMON_NAMES: [&str; 0] = [];
    const EXPECTED_SERVER_AUTH_EKU_DER: &[u8] = &[
        0x30, 0x0a, 0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01,
    ];

    #[tokio::test]
    async fn closed_window_is_zero_write_and_create_issuance_is_secret_safe_and_site_authoritative()
    -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
        let request = RequestFixture::new("hostile.device.example")?;
        let before = business_counts(&database).await?;
        let result = enrollment::intake(
            &database,
            gateway_signer.clone(),
            request.input.clone(),
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if result.err() != Some(EnrollmentError::ProvisioningWindowClosed)
            || business_counts(&database).await? != before
        {
            return Err(TestFailure::ClosedWindowWrote);
        }

        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        let issued = match enrollment::intake(
            &database,
            gateway_signer,
            request.input,
            SOURCE_IP,
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::IntakeFailed)?
        {
            EnrollmentOutcome::Issued(issued) => issued,
            EnrollmentOutcome::Pending(_) => return Err(TestFailure::UnexpectedOutcome),
        };
        let token = *issued.device_token.as_bytes();
        let leaf = issued.gateway_leaf_der.clone();
        let evidence = issuance_evidence(&database).await?;
        let expected_audit_detail = format!(
            "{{\"resolution\":\"create_device\",\"certificate_serial\":\"{}\",\"gateway_spki_sha256\":\"{}\"}}",
            evidence.certificate_serial,
            hex::encode(request.spki)
        );
        if evidence.devices != 1
            || evidence.requests != 1
            || evidence.tokens != 1
            || evidence.certificates != 1
            || evidence.active_certificates != 1
            || evidence.token_hash != Sha256::digest(token).as_slice()
            || evidence.certificate_spki != request.spki
            || evidence.request_state != "issued"
            || evidence.resolution != "create_device"
            || evidence.audit_actor != "device:enrollment"
            || evidence.audit_action != "issue_device_credentials"
            || evidence.audit_reason != "first_enrollment"
            || evidence.audit_detail != expected_audit_detail
        {
            return Err(TestFailure::IssuanceEvidenceChanged);
        }
        let certificate_der = CertificateDer::from(leaf.clone());
        let parsed = ParsedCertificate::try_from(&certificate_der)
            .map_err(|_| TestFailure::CertificateInvalid)?;
        let expected_name = ServerName::try_from("gateway.contest.example")
            .map_err(|_| TestFailure::CertificateInvalid)?;
        let hostile_name = ServerName::try_from("hostile.device.example")
            .map_err(|_| TestFailure::CertificateInvalid)?;
        let leaf_spki: [u8; 32] = Sha256::digest(parsed.subject_public_key_info().as_ref()).into();
        let leaf_profile = assert_exact_gateway_leaf_profile(&leaf)?;
        if verify_server_name(&parsed, &expected_name).is_err()
            || verify_server_name(&parsed, &hostile_name).is_ok()
            || leaf_spki != request.spki
            || leaf_profile.serial == request.serial_suggestion
        {
            return Err(TestFailure::CsrAuthorityEscaped);
        }
        let database_bytes = fixture.database_bytes()?;
        if contains_bytes(&database_bytes, &token) || contains_bytes(&database_bytes, &leaf) {
            return Err(TestFailure::PlaintextPersisted);
        }
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn pending_poll_is_idempotent_conflict_is_zero_write_and_close_expires_approved_claim()
    -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        let original = RequestFixture::new("first.invalid.example")?;
        let issued = expect_issued(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                original.input,
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let replacement = RequestFixture::new("replacement.invalid.example")?;
        let first_pending = expect_pending(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                replacement.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let counts_after_pending = business_counts(&database).await?;
        let expected_create_detail = format!(
            "{{\"resolution\":\"replace_device_credentials\",\"state\":\"pending\",\"gateway_spki_sha256\":\"{}\"}}",
            hex::encode(replacement.spki)
        );
        let second_pending = expect_pending(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                replacement.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        if first_pending != second_pending
            || business_counts(&database).await? != counts_after_pending
            || audit_shape(&database, "create_enrollment_request").await?
                != (
                    "device:enrollment".to_owned(),
                    "credential_replacement".to_owned(),
                    expected_create_detail,
                )
        {
            return Err(TestFailure::PendingReplayWrote);
        }
        let conflicting = RequestFixture::new("other.invalid.example")?;
        let before_conflict = complete_counts(&database).await?;
        let conflict = enrollment::intake(
            &database,
            gateway_signer.clone(),
            conflicting.input,
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if conflict.err() != Some(EnrollmentError::DeviceIdentityConflict)
            || complete_counts(&database).await? != before_conflict
        {
            return Err(TestFailure::ConflictWrote);
        }

        let credential_counts_before = credential_counts(&database).await?;
        enrollment::approve_request(&database, first_pending, correlation_id())
            .await
            .map_err(|_| TestFailure::DecisionFailed)?;
        if credential_counts(&database).await? != credential_counts_before
            || request_state(&database, first_pending).await? != "approved"
            || audit_shape(&database, "approve_enrollment_request").await?
                != (
                    "operator:self".to_owned(),
                    "operator_requested".to_owned(),
                    "{}".to_owned(),
                )
        {
            return Err(TestFailure::ApprovalIssued);
        }
        provisioning::close_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        if request_state(&database, first_pending).await? != "expired"
            || credential_counts(&database).await? != credential_counts_before
            || expiry_audit(&database).await? != (1, "window_closed".to_owned())
        {
            return Err(TestFailure::CloseDidNotExpire);
        }
        let poll = enrollment::intake(
            &database,
            gateway_signer,
            replacement.input,
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if poll.err() != Some(EnrollmentError::ProvisioningWindowClosed)
            || credential_counts(&database).await? != credential_counts_before
            || issued.device_id.to_string().is_empty()
        {
            return Err(TestFailure::ClosedClaimIssued);
        }
        Ok(())
    }

    #[tokio::test]
    async fn same_spki_retry_reissues_once_and_rejected_poll_is_terminal() -> Result<(), TestFailure>
    {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        let original = RequestFixture::new("ignored.original.example")?;
        let serial_suggestion = original.serial_suggestion;
        let first = expect_issued(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                original.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let first_token = *first.device_token.as_bytes();
        let second = expect_issued(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                original.input,
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let certificate_states = certificate_states(&database).await?;
        let first_serial = parsed_leaf_serial(&first.gateway_leaf_der)?;
        let second_serial = parsed_leaf_serial(&second.gateway_leaf_der)?;
        if first.device_id != second.device_id
            || first_token == *second.device_token.as_bytes()
            || first_serial == second_serial
            || first_serial == serial_suggestion
            || second_serial == serial_suggestion
            || certificate_states != (1, 1)
            || business_counts(&database).await?.requests != 2
            || latest_issuance_reason(&database).await? != "same_spki_retry"
        {
            return Err(TestFailure::SameSpkiRetryChanged);
        }

        let replacement = RequestFixture::new("ignored.rejected.example")?;
        let pending = expect_pending(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                replacement.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let credentials_before = credential_counts(&database).await?;
        enrollment::reject_request(&database, pending, correlation_id())
            .await
            .map_err(|_| TestFailure::DecisionFailed)?;
        let poll = enrollment::intake(
            &database,
            gateway_signer,
            replacement.input,
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if poll.err() != Some(EnrollmentError::RequestRejected)
            || credential_counts(&database).await? != credentials_before
            || request_state(&database, pending).await? != "rejected"
            || audit_shape(&database, "reject_enrollment_request").await?
                != (
                    "operator:self".to_owned(),
                    "operator_requested".to_owned(),
                    "{}".to_owned(),
                )
        {
            return Err(TestFailure::RejectedPollChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn csr_spki_mismatch_and_duplicate_issuance_audit_leave_zero_partial_state()
    -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
        let mut mismatch = RequestFixture::new("mismatch.invalid.example")?;
        mismatch.input.gateway_spki_sha256 = "00".repeat(32);
        let before = complete_counts(&database).await?;
        let mismatch_result = enrollment::intake(
            &database,
            gateway_signer.clone(),
            mismatch.input,
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if mismatch_result.err() != Some(EnrollmentError::SpkiMismatch)
            || complete_counts(&database).await? != before
        {
            return Err(TestFailure::MismatchWrote);
        }

        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        let request = RequestFixture::new("rollback.invalid.example")?;
        let duplicate_id = Uuid::now_v7();
        reserve_audit_id(&database, duplicate_id).await?;
        let before_duplicate = complete_counts(&database).await?;
        let result = intake_with_ids(
            &database,
            gateway_signer,
            request.validated(),
            correlation_id(),
            IntakeIds {
                request: Uuid::now_v7(),
                device: Uuid::now_v7(),
                certificate: Uuid::now_v7(),
                audit: AuditEventId::from_uuid(duplicate_id),
            },
        )
        .await;
        if !matches!(result, Err(EnrollmentStoreError::AuditInsertFailed))
            || complete_counts(&database).await? != before_duplicate
        {
            return Err(TestFailure::DuplicateAuditDidNotRollBack);
        }
        Ok(())
    }

    struct RequestFixture {
        input: EnrollmentRequestInput,
        csr_der: Vec<u8>,
        spki: [u8; 32],
        serial_suggestion: [u8; 20],
    }

    impl RequestFixture {
        fn new(hostile_san: &str) -> Result<Self, TestFailure> {
            let key = KeyPair::generate().map_err(|_| TestFailure::RequestFailed)?;
            let mut params = CertificateParams::new(vec![hostile_san.to_owned()])
                .map_err(|_| TestFailure::RequestFailed)?;
            let mut name = DistinguishedName::new();
            name.push(DnType::CommonName, "hostile-cn.invalid");
            name.push(
                DnType::CustomDnType(vec![2, 5, 4, 5]),
                hex::encode(HOSTILE_CSR_SERIAL_SUGGESTION),
            );
            params.distinguished_name = name;
            let csr = params
                .serialize_request(&key)
                .map_err(|_| TestFailure::RequestFailed)?;
            let csr_der = csr.der().to_vec();
            let spki: [u8; 32] = Sha256::digest(key.subject_public_key_info()).into();
            let machine_hardware_id =
                Uuid::new_v5(&Uuid::NAMESPACE_OID, b"natsume-enrollment-test-machine").to_string();
            Ok(Self {
                input: EnrollmentRequestInput {
                    machine_hardware_id,
                    hardware_identity_quality: "strong".to_owned(),
                    gateway_csr_der: encode_standard_base64(&csr_der),
                    gateway_spki_sha256: hex::encode(spki),
                    client_version: "2.0.0-test".to_owned(),
                    protocol_version: 1,
                },
                csr_der,
                spki,
                serial_suggestion: HOSTILE_CSR_SERIAL_SUGGESTION,
            })
        }

        fn validated(&self) -> ValidatedEnrollmentRequest {
            ValidatedEnrollmentRequest {
                machine_hardware_id: self.input.machine_hardware_id.clone(),
                hardware_identity_quality: HardwareIdentityQuality::Strong,
                gateway_csr_der: self.csr_der.clone(),
                gateway_spki_sha256: self.spki,
                client_version: self.input.client_version.clone(),
                protocol_version: self.input.protocol_version,
                source_ip: SOURCE_IP.to_string(),
            }
        }
    }

    struct ParsedLeafProfile {
        serial: Vec<u8>,
    }

    fn assert_exact_gateway_leaf_profile(
        leaf_der: &[u8],
    ) -> Result<ParsedLeafProfile, TestFailure> {
        let (remainder, certificate) =
            parse_x509_certificate(leaf_der).map_err(|_| TestFailure::CertificateInvalid)?;
        if !remainder.is_empty() {
            return Err(TestFailure::CertificateInvalid);
        }
        let subject_alt_name = certificate
            .subject_alternative_name()
            .map_err(|_| TestFailure::CertificateInvalid)?
            .ok_or(TestFailure::GatewayCertificateProfileChanged)?;
        let exact_site_san = matches!(
            subject_alt_name.value.general_names.as_slice(),
            [GeneralName::DNSName(name)] if *name == TEST_GATEWAY_HOSTNAME
        );
        let subject_common_names = certificate
            .subject()
            .iter_common_name()
            .map(|name| name.as_str().map_err(|_| TestFailure::CertificateInvalid))
            .collect::<Result<Vec<_>, _>>()?;
        let extended_key_usage = certificate
            .extended_key_usage()
            .map_err(|_| TestFailure::CertificateInvalid)?
            .ok_or(TestFailure::GatewayCertificateProfileChanged)?;
        let extended_key_usage_extension = certificate
            .get_extension_unique(&x509_parser::oid_registry::OID_X509_EXT_EXTENDED_KEY_USAGE)
            .map_err(|_| TestFailure::CertificateInvalid)?
            .ok_or(TestFailure::GatewayCertificateProfileChanged)?;
        let exact_server_auth = extended_key_usage.value.server_auth
            && extended_key_usage_extension.value == EXPECTED_SERVER_AUTH_EKU_DER;
        let site = GatewaySiteConfig::for_test(TEST_GATEWAY_HOSTNAME, TEST_GATEWAY_NOT_AFTER)
            .map_err(|_| TestFailure::CertificateInvalid)?;
        if !exact_site_san
            || subject_common_names.as_slice() != EXPECTED_SUBJECT_COMMON_NAMES
            || certificate.subject().iter_attributes().next().is_some()
            || !exact_server_auth
            || certificate.validity().not_after.timestamp()
                != site.gateway_not_after().unix_seconds()
        {
            return Err(TestFailure::GatewayCertificateProfileChanged);
        }
        let serial = certificate.raw_serial().to_vec();
        if serial.is_empty() || serial.iter().all(|byte| *byte == 0) {
            return Err(TestFailure::CertificateInvalid);
        }
        Ok(ParsedLeafProfile { serial })
    }

    fn parsed_leaf_serial(leaf_der: &[u8]) -> Result<Vec<u8>, TestFailure> {
        let (remainder, certificate) =
            parse_x509_certificate(leaf_der).map_err(|_| TestFailure::CertificateInvalid)?;
        if !remainder.is_empty() || certificate.raw_serial().is_empty() {
            return Err(TestFailure::CertificateInvalid);
        }
        Ok(certificate.raw_serial().to_vec())
    }

    fn expect_issued(
        result: Result<EnrollmentOutcome, EnrollmentError>,
    ) -> Result<enrollment::IssuedEnrollment, TestFailure> {
        match result.map_err(|_| TestFailure::IntakeFailed)? {
            EnrollmentOutcome::Issued(issued) => Ok(issued),
            EnrollmentOutcome::Pending(_) => Err(TestFailure::UnexpectedOutcome),
        }
    }

    fn expect_pending(
        result: Result<EnrollmentOutcome, EnrollmentError>,
    ) -> Result<Uuid, TestFailure> {
        match result.map_err(|_| TestFailure::IntakeFailed)? {
            EnrollmentOutcome::Pending(pending) => Ok(pending.enrollment_request_id),
            EnrollmentOutcome::Issued(_) => Err(TestFailure::UnexpectedOutcome),
        }
    }

    fn correlation_id() -> CorrelationId {
        CorrelationId::from_uuid(Uuid::now_v7())
    }

    async fn business_counts(database: &Database) -> Result<BusinessCounts, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT (SELECT COUNT(*) FROM devices) AS devices, \
                     (SELECT COUNT(*) FROM enrollment_requests) AS requests, \
                     (SELECT COUNT(*) FROM device_tokens) AS tokens, \
                     (SELECT COUNT(*) FROM gateway_certificates) AS certificates",
                )
                .get_result(connection)
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn complete_counts(database: &Database) -> Result<CompleteCounts, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT (SELECT COUNT(*) FROM devices) AS devices, \
                     (SELECT COUNT(*) FROM enrollment_requests) AS requests, \
                     (SELECT COUNT(*) FROM device_tokens) AS tokens, \
                     (SELECT COUNT(*) FROM gateway_certificates) AS certificates, \
                     (SELECT COUNT(*) FROM audit_events) AS audits",
                )
                .get_result(connection)
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn credential_counts(database: &Database) -> Result<(i64, i64), TestFailure> {
        let counts = business_counts(database).await?;
        Ok((counts.tokens, counts.certificates))
    }

    async fn issuance_evidence(database: &Database) -> Result<IssuanceEvidence, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT (SELECT COUNT(*) FROM devices) AS devices, \
                     (SELECT COUNT(*) FROM enrollment_requests) AS requests, \
                     (SELECT COUNT(*) FROM device_tokens) AS tokens, \
                     (SELECT COUNT(*) FROM gateway_certificates) AS certificates, \
                     (SELECT COUNT(*) FROM gateway_certificates WHERE status = 'active') \
                     AS active_certificates, dt.token_hash, gc.spki_sha256 AS certificate_spki, \
                     er.state AS request_state, er.resolution, ae.actor AS audit_actor, \
                     gc.serial AS certificate_serial, ae.action_kind AS audit_action, \
                     ae.reason_code AS audit_reason, \
                     ae.redacted_detail_json AS audit_detail FROM enrollment_requests er \
                     JOIN device_tokens dt ON dt.enrollment_request_id = er.enrollment_request_id \
                     JOIN gateway_certificates gc ON gc.enrollment_request_id = er.enrollment_request_id \
                     JOIN audit_events ae ON ae.audit_event_id = er.issuance_audit_event_id",
                )
                .get_result(connection)
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn request_state(database: &Database, request_id: Uuid) -> Result<String, TestFailure> {
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "SELECT state AS value FROM enrollment_requests WHERE enrollment_request_id = ?",
                )
                .bind::<Text, _>(request_id.to_string())
                .get_result::<StringRow>(connection)
                .map(|row| row.value)
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn certificate_states(database: &Database) -> Result<(i64, i64), TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT SUM(status = 'active') AS active, SUM(status = 'retired') AS retired \
                     FROM gateway_certificates",
                )
                .get_result::<CertificateStateCounts>(connection)
                .map(|row| (row.active, row.retired))
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn latest_issuance_reason(database: &Database) -> Result<String, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT reason_code AS value FROM audit_events \
                     WHERE action_kind = 'issue_device_credentials' ORDER BY rowid DESC LIMIT 1",
                )
                .get_result::<StringRow>(connection)
                .map(|row| row.value)
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn audit_shape(
        database: &Database,
        action: &'static str,
    ) -> Result<(String, String, String), TestFailure> {
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "SELECT actor, reason_code AS reason, redacted_detail_json AS detail \
                     FROM audit_events WHERE action_kind = ? ORDER BY rowid DESC LIMIT 1",
                )
                .bind::<Text, _>(action)
                .get_result::<AuditShape>(connection)
                .map(|row| (row.actor, row.reason, row.detail))
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn expiry_audit(database: &Database) -> Result<(i64, String), TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT CAST(json_extract(redacted_detail_json, '$.expired_count') AS INTEGER) \
                     AS expired_count, reason_code AS reason FROM audit_events \
                     WHERE action_kind = 'expire_enrollment_requests' ORDER BY rowid DESC LIMIT 1",
                )
                .get_result::<ExpiryAudit>(connection)
                .map(|row| (row.expired_count, row.reason))
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn reserve_audit_id(database: &Database, id: Uuid) -> Result<(), TestFailure> {
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "INSERT INTO audit_events (audit_event_id, occurred_at, actor, action_kind, \
                     resource_type, resource_id, result, reason_code, correlation_id, \
                     group_correlation_id, redacted_detail_json) VALUES (?, \
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'system:test', 'reserved_test_audit', \
                     'test', NULL, 'succeeded', NULL, ?, NULL, '{}')",
                )
                .bind::<Text, _>(id.to_string())
                .bind::<Text, _>(Uuid::now_v7().to_string())
                .execute(connection)
                .map(|_| ())
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window == needle)
    }

    #[derive(Debug, PartialEq, Eq, QueryableByName)]
    struct BusinessCounts {
        #[diesel(sql_type = BigInt)]
        devices: i64,
        #[diesel(sql_type = BigInt)]
        requests: i64,
        #[diesel(sql_type = BigInt)]
        tokens: i64,
        #[diesel(sql_type = BigInt)]
        certificates: i64,
    }

    #[derive(Debug, PartialEq, Eq, QueryableByName)]
    struct CompleteCounts {
        #[diesel(sql_type = BigInt)]
        devices: i64,
        #[diesel(sql_type = BigInt)]
        requests: i64,
        #[diesel(sql_type = BigInt)]
        tokens: i64,
        #[diesel(sql_type = BigInt)]
        certificates: i64,
        #[diesel(sql_type = BigInt)]
        audits: i64,
    }

    #[derive(QueryableByName)]
    struct IssuanceEvidence {
        #[diesel(sql_type = BigInt)]
        devices: i64,
        #[diesel(sql_type = BigInt)]
        requests: i64,
        #[diesel(sql_type = BigInt)]
        tokens: i64,
        #[diesel(sql_type = BigInt)]
        certificates: i64,
        #[diesel(sql_type = BigInt)]
        active_certificates: i64,
        #[diesel(sql_type = Binary)]
        token_hash: Vec<u8>,
        #[diesel(sql_type = Binary)]
        certificate_spki: Vec<u8>,
        #[diesel(sql_type = Text)]
        request_state: String,
        #[diesel(sql_type = Text)]
        resolution: String,
        #[diesel(sql_type = Text)]
        audit_actor: String,
        #[diesel(sql_type = Text)]
        certificate_serial: String,
        #[diesel(sql_type = Text)]
        audit_action: String,
        #[diesel(sql_type = Text)]
        audit_reason: String,
        #[diesel(sql_type = Text)]
        audit_detail: String,
    }

    #[derive(QueryableByName)]
    struct StringRow {
        #[diesel(sql_type = Text)]
        value: String,
    }

    #[derive(QueryableByName)]
    struct CertificateStateCounts {
        #[diesel(sql_type = BigInt)]
        active: i64,
        #[diesel(sql_type = BigInt)]
        retired: i64,
    }

    #[derive(QueryableByName)]
    struct AuditShape {
        #[diesel(sql_type = Text)]
        actor: String,
        #[diesel(sql_type = Text)]
        reason: String,
        #[diesel(sql_type = Text)]
        detail: String,
    }

    #[derive(QueryableByName)]
    struct ExpiryAudit {
        #[diesel(sql_type = BigInt)]
        expired_count: i64,
        #[diesel(sql_type = Text)]
        reason: String,
    }

    struct DatabaseFixture {
        path: PathBuf,
    }

    impl DatabaseFixture {
        fn new() -> Self {
            Self {
                path: std::env::temp_dir().join(format!(
                    "natsume-enrollment-test-{}.sqlite3",
                    Uuid::now_v7()
                )),
            }
        }

        async fn connect(&self) -> Result<Database, TestFailure> {
            Database::connect_and_migrate(&DatabaseConfig::new(&self.path, true))
                .await
                .map_err(|_| TestFailure::DatabaseFailed)
        }

        fn database_bytes(&self) -> Result<Vec<u8>, TestFailure> {
            let mut bytes = fs::read(&self.path).map_err(|_| TestFailure::EvidenceFailed)?;
            let wal = PathBuf::from(format!("{}-wal", self.path.display()));
            if wal.exists() {
                bytes.extend(fs::read(wal).map_err(|_| TestFailure::EvidenceFailed)?);
            }
            Ok(bytes)
        }
    }

    impl Drop for DatabaseFixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_file(format!("{}-wal", self.path.display()));
            let _ = fs::remove_file(format!("{}-shm", self.path.display()));
        }
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the test database failed"))]
        DatabaseFailed,
        #[snafu(display("the test issuer failed"))]
        IssuerFailed,
        #[snafu(display("the test request failed"))]
        RequestFailed,
        #[snafu(display("the provisioning window mutation failed"))]
        WindowMutationFailed,
        #[snafu(display("Enrollment intake failed"))]
        IntakeFailed,
        #[snafu(display("the Enrollment outcome was unexpected"))]
        UnexpectedOutcome,
        #[snafu(display("the closed window wrote state"))]
        ClosedWindowWrote,
        #[snafu(display("the issuance evidence changed"))]
        IssuanceEvidenceChanged,
        #[snafu(display("the issued certificate is invalid"))]
        CertificateInvalid,
        #[snafu(display("the issued Gateway certificate profile changed"))]
        GatewayCertificateProfileChanged,
        #[snafu(display("CSR authority escaped into the leaf"))]
        CsrAuthorityEscaped,
        #[snafu(display("issuance plaintext was persisted"))]
        PlaintextPersisted,
        #[snafu(display("the pending replay wrote state"))]
        PendingReplayWrote,
        #[snafu(display("a different-SPKI conflict wrote state"))]
        ConflictWrote,
        #[snafu(display("approval issued credentials"))]
        ApprovalIssued,
        #[snafu(display("window close did not expire the request"))]
        CloseDidNotExpire,
        #[snafu(display("a closed claim issued credentials"))]
        ClosedClaimIssued,
        #[snafu(display("same-SPKI retry semantics changed"))]
        SameSpkiRetryChanged,
        #[snafu(display("rejected polling semantics changed"))]
        RejectedPollChanged,
        #[snafu(display("the operator decision failed"))]
        DecisionFailed,
        #[snafu(display("a CSR/SPKI mismatch wrote state"))]
        MismatchWrote,
        #[snafu(display("a duplicate audit did not roll back issuance"))]
        DuplicateAuditDidNotRollBack,
        #[snafu(display("database evidence failed"))]
        EvidenceFailed,
    }
}
