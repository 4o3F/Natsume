use diesel::{OptionalExtension, RunQueryDsl, sql_types::Text, sqlite::SqliteConnection};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    application::enrollment::{
        DeviceConnectionEvictor, EnrollmentError, EnrollmentOutcome, EnrollmentResolution,
        EnrollmentState, GatewayIssuer, IssuanceReason, PendingEnrollment,
        ValidatedEnrollmentRequest,
    },
    audit::{self, AuditEvent, AuditEventId, CorrelationId},
    db::Database,
};

use super::{
    EnrollmentStoreError, MAX_LIVE_ENROLLMENT_REQUESTS,
    issuance::{
        insert_request, issuance_device_context, issue_existing_request, issue_new_device,
        issue_same_spki_replacement,
    },
    row::{
        CountRow, CurrentCredentialFactsRow, CurrentCredentialsRow, DeviceRow, LiveRequestRow,
        PersistedDeviceRow, ReplacementDeviceState, StateRow, canonical_uuid_v7,
    },
};

pub(crate) async fn intake<E>(
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
    .map_err(EnrollmentError::from)
}

#[derive(Clone, Copy)]
pub(super) struct IntakeIds {
    pub(super) request: Uuid,
    pub(super) device: Uuid,
    pub(super) certificate: Uuid,
    pub(super) audit: AuditEventId,
}

#[cfg(test)]
pub(super) async fn intake_with_ids(
    database: &Database,
    issuer: GatewayIssuer,
    request: ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    ids: IntakeIds,
) -> Result<EnrollmentOutcome, EnrollmentStoreError> {
    intake_with_ids_and_connection_eviction(
        database,
        issuer,
        request,
        correlation_id,
        ids,
        crate::application::enrollment::NoLiveDeviceConnections,
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
) -> Result<EnrollmentOutcome, EnrollmentStoreError>
where
    E: DeviceConnectionEvictor,
{
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                intake_in_transaction(
                    connection,
                    &issuer,
                    &request,
                    correlation_id,
                    ids,
                    &connection_evictor,
                )
            })
        })
        .await
        .map_err(|_| EnrollmentStoreError::AcquireFailed)?
}

fn intake_in_transaction<E>(
    connection: &mut SqliteConnection,
    issuer: &GatewayIssuer,
    request: &ValidatedEnrollmentRequest,
    correlation_id: CorrelationId,
    ids: IntakeIds,
    connection_evictor: &E,
) -> Result<EnrollmentOutcome, EnrollmentStoreError>
where
    E: DeviceConnectionEvictor,
{
    require_open_window(connection)?;
    let device = read_device(connection, &request.machine_hardware_id)?;
    if latest_request_state(connection, &request.machine_hardware_id)?.as_deref()
        == Some("rejected")
    {
        return Err(EnrollmentStoreError::RequestRejected);
    }
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
            "approved" => {
                let has_current_credentials =
                    read_current_credentials(connection, device.device_id)?.is_some();
                let issuance_context =
                    issuance_device_context(device.state, has_current_credentials)?;
                issue_existing_request(
                    connection,
                    issuer,
                    request,
                    correlation_id,
                    request_id,
                    device.device_id,
                    ids.certificate,
                    ids.audit,
                    IssuanceReason::CredentialReplacement,
                    issuance_context,
                    connection_evictor,
                )
            }
            _ => Err(EnrollmentStoreError::InvalidPersistedFacts),
        };
    }

    require_live_request_capacity(connection)?;

    let Some(device) = device else {
        return issue_new_device(connection, issuer, request, correlation_id, ids);
    };
    validate_device_for_replacement(&device, request)?;
    let current = read_current_credentials(connection, device.device_id)?;
    validate_current_credentials(device.state, current.as_ref())?;
    if device.state == ReplacementDeviceState::Enrolled {
        let current = current
            .as_ref()
            .ok_or(EnrollmentStoreError::InvalidPersistedFacts)?;
        if same_digest(&current.gateway_spki_sha256, &request.gateway_spki_sha256)? {
            return issue_same_spki_replacement(
                connection,
                issuer,
                request,
                correlation_id,
                device.device_id,
                ids,
                connection_evictor,
            );
        }
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

pub(super) fn require_open_window(
    connection: &mut SqliteConnection,
) -> Result<(), EnrollmentStoreError> {
    let row = diesel::sql_query("SELECT state FROM provisioning_window WHERE singleton = 1")
        .get_result::<StateRow>(connection)
        .map_err(|_| EnrollmentStoreError::WindowReadFailed)?;
    match row.state.as_str() {
        "open" => Ok(()),
        "closed" => Err(EnrollmentStoreError::ProvisioningWindowClosed),
        _ => Err(EnrollmentStoreError::InvalidPersistedFacts),
    }
}

pub(super) fn read_device(
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
            state: ReplacementDeviceState::from_persisted(&row.state)?,
        })
    })
    .transpose()
}

pub(super) fn read_live_requests(
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

pub(super) fn latest_request_state(
    connection: &mut SqliteConnection,
    machine_hardware_id: &str,
) -> Result<Option<String>, EnrollmentStoreError> {
    diesel::sql_query(
        "SELECT state FROM enrollment_requests WHERE machine_hardware_id = ? \
         ORDER BY rowid DESC LIMIT 1",
    )
    .bind::<Text, _>(machine_hardware_id)
    .get_result::<StateRow>(connection)
    .optional()
    .map(|row| row.map(|row| row.state))
    .map_err(|_| EnrollmentStoreError::RequestReadFailed)
}

pub(super) fn validate_replacement_device<'a>(
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

pub(super) fn validate_device_for_replacement(
    device: &DeviceRow,
    request: &ValidatedEnrollmentRequest,
) -> Result<(), EnrollmentStoreError> {
    if device.hardware_identity_quality != request.hardware_identity_quality.as_persisted() {
        return Err(EnrollmentStoreError::DeviceIdentityConflict);
    }
    Ok(())
}

pub(super) fn validate_current_credentials(
    device_state: ReplacementDeviceState,
    current: Option<&CurrentCredentialsRow>,
) -> Result<(), EnrollmentStoreError> {
    match (device_state, current) {
        (ReplacementDeviceState::Enrolled | ReplacementDeviceState::Disabled, Some(_))
        | (ReplacementDeviceState::Revoked, None) => Ok(()),
        (ReplacementDeviceState::Enrolled | ReplacementDeviceState::Disabled, None)
        | (ReplacementDeviceState::Revoked, Some(_)) => {
            Err(EnrollmentStoreError::InvalidPersistedFacts)
        }
    }
}

pub(super) fn require_live_request_capacity(
    connection: &mut SqliteConnection,
) -> Result<(), EnrollmentStoreError> {
    let live_count = diesel::sql_query(
        "SELECT COUNT(*) AS value FROM enrollment_requests \
         WHERE state IN ('pending', 'approved')",
    )
    .get_result::<CountRow>(connection)
    .map_err(|_| EnrollmentStoreError::RequestReadFailed)?
    .value;
    if live_count >= MAX_LIVE_ENROLLMENT_REQUESTS {
        return Err(EnrollmentStoreError::LiveRequestCapacityExceeded);
    }
    Ok(())
}

pub(super) fn read_current_credentials(
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

pub(super) fn same_digest(
    persisted: &[u8],
    presented: &[u8; 32],
) -> Result<bool, EnrollmentStoreError> {
    if persisted.len() != 32 {
        return Err(EnrollmentStoreError::InvalidPersistedFacts);
    }
    Ok(bool::from(persisted.ct_eq(presented.as_slice())))
}

pub(super) fn create_pending_request(
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
