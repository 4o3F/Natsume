//! Truth-reading persistence operations for provisioning and credential issuance.

use snafu::Snafu;
use sqlx::{Acquire, Sqlite, SqliteConnection, Transaction, query, query_as, query_scalar};
use uuid::Uuid;

pub use super::domain_checks::ProvisioningWindowChange;
use super::domain_checks::{
    AuditActionKind, AuditBacking, AuditResourceType, AuditResult, DomainCheckError,
    EnrollmentIssuanceBinding, EnrollmentResolution, EnrollmentState,
    PROVISIONING_WINDOW_RESOURCE_ID, ProvisioningWindow, ProvisioningWindowState,
    check_device_token_insert, check_enrollment_issuance, check_enrollment_issuance_window,
    check_gateway_certificate_insert, check_provisioning_window_transition,
    check_recovery_provisioning_window_transition,
};

const RECOVERY_ACTOR: &str = "system:recovery";
const RECOVERY_ACTION: &str = "recovery_close_provisioning_window";
const RECOVERY_REASON_CODE: &str = "recovery_fail_closed";
const RECOVERY_REDACTED_DETAIL_JSON: &str = r#"{"from_state":"open","to_state":"closed"}"#;
const RECOVERY_TIMESTAMP_SQL: &str = "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

type EnrollmentRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Vec<u8>,
    Option<String>,
);
type AuditRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
    String,
);

/// Database or policy failure from a guarded persistence operation.
#[derive(Debug, Snafu)]
pub enum GuardedWriteError {
    #[snafu(display("guarded persistence query failed"))]
    Database { source: sqlx::Error },

    #[snafu(display("guarded persistence policy rejected the write: {source}"))]
    DomainCheck { source: DomainCheckError },

    #[snafu(display(
        "provisioning-window compare-and-swap must affect exactly one row, affected {affected_rows}"
    ))]
    ProvisioningWindowCompareAndSwap { affected_rows: u64 },
}

/// A phase-specific failure while closing an open provisioning window during recovery.
///
/// The caller owns the surrounding `BEGIN IMMEDIATE` transaction and must not commit it after an
/// error. Dropping or rolling back that transaction removes any recovery audit inserted before a
/// failed compare-and-swap.
#[derive(Debug, Snafu)]
pub enum RecoveryCloseError {
    #[snafu(display("failed to read the provisioning-window singleton for recovery"))]
    Read { source: GuardedWriteError },

    #[snafu(display("failed to write or verify the recovery audit"))]
    Audit { source: GuardedWriteError },

    #[snafu(display("failed to compare-and-swap the provisioning-window singleton for recovery"))]
    CompareAndSwap { source: GuardedWriteError },
}

/// Audited Device Token and Gateway certificate issuance from one approved Enrollment request.
#[derive(Clone, Copy)]
pub struct EnrollmentCredentials<'a> {
    pub enrollment_request_id: &'a str,
    pub device_pk: &'a str,
    pub issuing_actor: &'a str,
    pub audit_event_id: &'a str,
    pub occurred_at: &'a str,
    pub correlation_id: &'a str,
    pub reason_code: Option<&'a str>,
    pub redacted_detail_json: &'a str,
    pub token_hash: &'a [u8],
    pub certificate_id: &'a str,
    pub certificate_serial: &'a str,
    pub certificate_spki_sha256: &'a [u8],
    pub certificate_not_after: &'a str,
}

struct PersistedEnrollment {
    enrollment_request_id: String,
    machine_hardware_id: String,
    state: EnrollmentState,
    resolution: Option<EnrollmentResolution>,
    resolved_device_pk: Option<String>,
    gateway_spki_sha256: Vec<u8>,
    issuance_audit_event_id: Option<String>,
}

struct PersistedAudit {
    audit_event_id: String,
    occurred_at: String,
    actor: String,
    action_kind: AuditActionKind,
    resource_type: AuditResourceType,
    resource_id: String,
    result: AuditResult,
    reason_code: Option<String>,
    correlation_id: String,
    redacted_detail_json: String,
}

/// Changes the current provisioning-window singleton after reading its current state and audit row.
///
/// This function inserts the audit row itself inside a nested guarded savepoint, so a previously
/// committed audit cannot be replayed as backing for a new mutation. Its partial writes roll back
/// without committing or rolling back the caller's larger transaction.
///
/// # Errors
///
/// Returns [`GuardedWriteError`] when persisted truth cannot be read, policy rejects the
/// transition, or the singleton compare-and-swap cannot affect exactly one row.
pub async fn change_provisioning_window(
    transaction: &mut Transaction<'_, Sqlite>,
    change: ProvisioningWindowChange<'_>,
) -> Result<ProvisioningWindow, GuardedWriteError> {
    let mut guarded = transaction.begin().await.map_err(database_error)?;
    let result = async {
        insert_provisioning_window_audit(&mut guarded, change).await?;
        change_current_provisioning_window(&mut guarded, change).await
    }
    .await;
    match result {
        Ok(window) => {
            guarded.commit().await.map_err(database_error)?;
            Ok(window)
        }
        Err(error) => Err(error),
    }
}

/// Inserts a new Device Token and Gateway certificate as one guarded issuance operation.
///
/// The Enrollment request and resolved Device must already be visible in the caller's transaction.
/// This function inserts the issuance audit, transitions the request to `issued`, and inserts both
/// credential rows in one nested savepoint.
///
/// # Errors
///
/// Returns [`GuardedWriteError`] when persisted truth or either credential row is invalid.
pub async fn issue_enrollment_credentials(
    transaction: &mut Transaction<'_, Sqlite>,
    credentials: EnrollmentCredentials<'_>,
) -> Result<(), GuardedWriteError> {
    let mut guarded = transaction.begin().await.map_err(database_error)?;
    let result = write_enrollment_credentials(
        &mut guarded,
        credentials,
        EnrollmentResolution::CreateDevice,
    )
    .await;
    match result {
        Ok(()) => guarded.commit().await.map_err(database_error),
        Err(error) => Err(error),
    }
}

/// Atomically audits issuance, transitions the Enrollment, retires active credentials, and
/// inserts their replacements.
///
/// The caller must roll back its transaction when this function returns an error.
///
/// # Errors
///
/// Returns [`GuardedWriteError`] when persisted truth is invalid, active credentials are missing,
/// or any retirement or insertion fails.
pub async fn replace_enrollment_credentials(
    transaction: &mut Transaction<'_, Sqlite>,
    credentials: EnrollmentCredentials<'_>,
) -> Result<(), GuardedWriteError> {
    let mut guarded = transaction.begin().await.map_err(database_error)?;
    let result = write_enrollment_credentials(
        &mut guarded,
        credentials,
        EnrollmentResolution::ReplaceDeviceCredentials,
    )
    .await;
    match result {
        Ok(()) => guarded.commit().await.map_err(database_error),
        Err(error) => Err(error),
    }
}

/// Fails closed after migration or restore by closing an observed open singleton once.
///
/// The caller must supply the root transaction started with `BEGIN IMMEDIATE`. A closed singleton
/// returns `false` without inserting an audit or updating the row. An open singleton receives one
/// recovery audit and a state-and-revision CAS in that same transaction.
///
/// # Errors
///
/// Returns [`RecoveryCloseError`] with the failed read, audit, or compare-and-swap phase.
pub(crate) async fn close_provisioning_window_for_recovery(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<bool, RecoveryCloseError> {
    let current = read_current_provisioning_window(&mut *transaction)
        .await
        .map_err(|source| RecoveryCloseError::Read { source })?;
    if current.state == ProvisioningWindowState::Closed {
        return Ok(false);
    }

    let next_revision =
        current
            .revision
            .checked_add(1)
            .ok_or_else(|| RecoveryCloseError::Read {
                source: domain_error(DomainCheckError::ProvisioningWindowRevisionOverflow),
            })?;
    let audit_event_id = Uuid::now_v7().to_string();
    let occurred_at = recovery_timestamp(&mut *transaction)
        .await
        .map_err(|source| RecoveryCloseError::Audit { source })?;
    let change = ProvisioningWindowChange {
        state: ProvisioningWindowState::Closed,
        changed_by: RECOVERY_ACTOR,
        audit_event_id: &audit_event_id,
        correlation_id: &audit_event_id,
        reason_code: Some(RECOVERY_REASON_CODE),
        redacted_detail_json: RECOVERY_REDACTED_DETAIL_JSON,
        occurred_at: &occurred_at,
    };
    insert_recovery_audit(&mut *transaction, change)
        .await
        .map_err(|source| RecoveryCloseError::Audit { source })?;
    let audit = read_audit(&mut *transaction, change.audit_event_id)
        .await
        .map_err(|source| RecoveryCloseError::Audit { source })?;
    check_recovery_provisioning_window_transition(
        current,
        change,
        audit.as_ref().map(PersistedAudit::as_backing),
    )
    .map_err(domain_error)
    .map_err(|source| RecoveryCloseError::Audit { source })?;
    let next = ProvisioningWindow {
        state: change.state,
        revision: next_revision,
    };
    compare_and_swap_provisioning_window(&mut *transaction, current, next, change)
        .await
        .map_err(|source| RecoveryCloseError::CompareAndSwap { source })?;
    Ok(true)
}

async fn change_current_provisioning_window(
    connection: &mut SqliteConnection,
    change: ProvisioningWindowChange<'_>,
) -> Result<ProvisioningWindow, GuardedWriteError> {
    let current = read_current_provisioning_window(connection).await?;
    let next_revision = current
        .revision
        .checked_add(1)
        .ok_or_else(|| domain_error(DomainCheckError::ProvisioningWindowRevisionOverflow))?;
    let audit = read_audit(connection, change.audit_event_id).await?;
    check_provisioning_window_transition(
        current,
        change,
        audit.as_ref().map(PersistedAudit::as_backing),
    )
    .map_err(domain_error)?;
    let next = ProvisioningWindow {
        state: change.state,
        revision: next_revision,
    };
    compare_and_swap_provisioning_window(connection, current, next, change).await?;
    Ok(next)
}

/// Performs the singleton state-and-revision compare-and-swap used by normal and recovery writes.
///
/// Callers must validate the target state, target revision, and audit backing before calling this
/// function. A result other than exactly one affected row is a failed CAS, never a silent no-op.
pub(crate) async fn compare_and_swap_provisioning_window(
    connection: &mut SqliteConnection,
    current: ProvisioningWindow,
    next: ProvisioningWindow,
    change: ProvisioningWindowChange<'_>,
) -> Result<(), GuardedWriteError> {
    let outcome = query(
        "UPDATE provisioning_window SET state = ?, revision = ?, last_audit_event_id = ? WHERE singleton = 1 AND state = ? AND revision = ?",
    )
    .bind(window_state_text(next.state))
    .bind(next.revision)
    .bind(change.audit_event_id)
    .bind(window_state_text(current.state))
    .bind(current.revision)
    .execute(connection)
    .await
    .map_err(database_error)?;
    if outcome.rows_affected() != 1 {
        return Err(GuardedWriteError::ProvisioningWindowCompareAndSwap {
            affected_rows: outcome.rows_affected(),
        });
    }
    Ok(())
}

/// Reads the authoritative current provisioning-window singleton.
pub(crate) async fn read_current_provisioning_window(
    connection: &mut SqliteConnection,
) -> Result<ProvisioningWindow, GuardedWriteError> {
    let row = query_as::<_, (String, i64)>(
        "SELECT state, revision FROM provisioning_window WHERE singleton = 1",
    )
    .fetch_optional(connection)
    .await
    .map_err(database_error)?;
    let Some((state, revision)) = row else {
        return Err(domain_error(
            DomainCheckError::PersistedProvisioningWindowInvalid,
        ));
    };
    if revision < 0 {
        return Err(domain_error(
            DomainCheckError::PersistedProvisioningWindowInvalid,
        ));
    }
    Ok(ProvisioningWindow {
        state: parse_window_state(&state)?,
        revision,
    })
}

async fn write_enrollment_credentials(
    connection: &mut SqliteConnection,
    credentials: EnrollmentCredentials<'_>,
    required_resolution: EnrollmentResolution,
) -> Result<(), GuardedWriteError> {
    let window = read_current_provisioning_window(connection).await?;
    check_enrollment_issuance_window(window.state).map_err(domain_error)?;

    let enrollment = read_enrollment(connection, credentials.enrollment_request_id).await?;
    let Some(enrollment) = enrollment else {
        return Err(domain_error(DomainCheckError::ApprovedEnrollmentRequired));
    };
    if enrollment.state != EnrollmentState::Approved
        || enrollment.resolution.is_some()
        || enrollment.resolved_device_pk.is_some()
        || enrollment.issuance_audit_event_id.is_some()
    {
        return Err(domain_error(DomainCheckError::ApprovedEnrollmentRequired));
    }
    validate_machine_identity(connection, credentials.device_pk, &enrollment).await?;

    insert_enrollment_audit(connection, credentials, required_resolution).await?;
    transition_enrollment_to_issued(connection, credentials, required_resolution).await?;

    let enrollment = read_enrollment(connection, credentials.enrollment_request_id).await?;
    let Some(enrollment) = enrollment else {
        return Err(domain_error(DomainCheckError::IssuedEnrollmentRequired));
    };
    let Some(resolution) = enrollment.resolution else {
        return Err(domain_error(DomainCheckError::IssuedEnrollmentRequired));
    };
    validate_resolution(resolution, required_resolution)?;
    let Some(issuance_audit_event_id) = enrollment.issuance_audit_event_id.as_deref() else {
        return Err(domain_error(DomainCheckError::AuditBackingMissing));
    };
    let audit = read_audit(connection, issuance_audit_event_id).await?;
    check_enrollment_issuance(
        window.state,
        &enrollment.enrollment_request_id,
        issuance_audit_event_id,
        resolution,
        credentials.issuing_actor,
        audit.as_ref().map(PersistedAudit::as_backing),
    )
    .map_err(domain_error)?;
    validate_enrollment_bindings(window.state, credentials, &enrollment)?;

    if required_resolution == EnrollmentResolution::ReplaceDeviceCredentials {
        retire_active_credentials(connection, credentials.device_pk, credentials.token_hash)
            .await?;
    }
    insert_credentials(connection, credentials, &enrollment).await
}

async fn insert_enrollment_audit(
    connection: &mut SqliteConnection,
    credentials: EnrollmentCredentials<'_>,
    resolution: EnrollmentResolution,
) -> Result<(), GuardedWriteError> {
    let action_kind = match resolution {
        EnrollmentResolution::CreateDevice => "issue_device_enrollment",
        EnrollmentResolution::ReplaceDeviceCredentials => "replace_device_enrollment",
    };
    query(
        "INSERT INTO audit_events(audit_event_id, occurred_at, actor, action_kind, resource_type, resource_id, result, reason_code, correlation_id, redacted_detail_json) VALUES (?, ?, ?, ?, 'enrollment_request', ?, 'succeeded', ?, ?, ?)",
    )
    .bind(credentials.audit_event_id)
    .bind(credentials.occurred_at)
    .bind(credentials.issuing_actor)
    .bind(action_kind)
    .bind(credentials.enrollment_request_id)
    .bind(credentials.reason_code)
    .bind(credentials.correlation_id)
    .bind(credentials.redacted_detail_json)
    .execute(connection)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn transition_enrollment_to_issued(
    connection: &mut SqliteConnection,
    credentials: EnrollmentCredentials<'_>,
    resolution: EnrollmentResolution,
) -> Result<(), GuardedWriteError> {
    let outcome = query(
        "UPDATE enrollment_requests SET state = 'issued', resolution = ?, resolved_device_pk = ?, issuance_audit_event_id = ? WHERE enrollment_request_id = ? AND state = 'approved' AND resolution IS NULL AND resolved_device_pk IS NULL AND issuance_audit_event_id IS NULL",
    )
    .bind(resolution_text(resolution))
    .bind(credentials.device_pk)
    .bind(credentials.audit_event_id)
    .bind(credentials.enrollment_request_id)
    .execute(connection)
    .await
    .map_err(database_error)?;
    if outcome.rows_affected() != 1 {
        return Err(domain_error(DomainCheckError::ApprovedEnrollmentRequired));
    }
    Ok(())
}

fn validate_enrollment_bindings(
    window: ProvisioningWindowState,
    credentials: EnrollmentCredentials<'_>,
    enrollment: &PersistedEnrollment,
) -> Result<(), GuardedWriteError> {
    let binding = EnrollmentIssuanceBinding {
        enrollment_request_id: &enrollment.enrollment_request_id,
        state: enrollment.state,
        resolved_device_pk: enrollment.resolved_device_pk.as_deref(),
        gateway_spki_sha256: &enrollment.gateway_spki_sha256,
    };
    check_device_token_insert(
        window,
        credentials.device_pk,
        credentials.enrollment_request_id,
        Some(binding),
    )
    .map_err(domain_error)?;
    check_gateway_certificate_insert(
        window,
        credentials.device_pk,
        credentials.enrollment_request_id,
        credentials.certificate_spki_sha256,
        Some(binding),
    )
    .map_err(domain_error)
}

async fn validate_machine_identity(
    connection: &mut SqliteConnection,
    device_pk: &str,
    enrollment: &PersistedEnrollment,
) -> Result<(), GuardedWriteError> {
    let device = query_as::<_, (String, String)>(
        "SELECT machine_hardware_id, state FROM devices WHERE device_pk = ?",
    )
    .bind(device_pk)
    .fetch_optional(connection)
    .await
    .map_err(database_error)?;
    let Some((device_machine_hardware_id, device_state)) = device else {
        return Err(domain_error(DomainCheckError::EnrollmentDeviceMismatch));
    };
    if device_state != "enrolled" {
        return Err(domain_error(DomainCheckError::EnrollmentDeviceNotEnrolled));
    }
    if enrollment.machine_hardware_id != device_machine_hardware_id {
        return Err(domain_error(
            DomainCheckError::EnrollmentMachineHardwareIdMismatch,
        ));
    }
    Ok(())
}

async fn read_enrollment(
    connection: &mut SqliteConnection,
    enrollment_request_id: &str,
) -> Result<Option<PersistedEnrollment>, GuardedWriteError> {
    let row = query_as::<_, EnrollmentRow>(
        "SELECT enrollment_request_id, machine_hardware_id, state, resolution, resolved_device_pk, gateway_spki_sha256, issuance_audit_event_id FROM enrollment_requests WHERE enrollment_request_id = ?",
    )
    .bind(enrollment_request_id)
    .fetch_optional(connection)
    .await
    .map_err(database_error)?;
    row.map(parse_enrollment).transpose()
}

fn parse_enrollment(row: EnrollmentRow) -> Result<PersistedEnrollment, GuardedWriteError> {
    let (id, machine_id, state, resolution, device_pk, spki, audit_id) = row;
    let state = parse_enrollment_state(&state)?;
    let resolution = match resolution {
        Some(value) => Some(
            parse_resolution(&value)
                .ok_or_else(|| domain_error(DomainCheckError::IssuedEnrollmentRequired))?,
        ),
        None => None,
    };
    Ok(PersistedEnrollment {
        enrollment_request_id: id,
        machine_hardware_id: machine_id,
        state,
        resolution,
        resolved_device_pk: device_pk,
        gateway_spki_sha256: spki,
        issuance_audit_event_id: audit_id,
    })
}

async fn read_audit(
    connection: &mut SqliteConnection,
    audit_event_id: &str,
) -> Result<Option<PersistedAudit>, GuardedWriteError> {
    let row = query_as::<_, AuditRow>(
        "SELECT audit_event_id, occurred_at, actor, action_kind, resource_type, resource_id, result, reason_code, correlation_id, redacted_detail_json FROM audit_events WHERE audit_event_id = ?",
    )
    .bind(audit_event_id)
    .fetch_optional(connection)
    .await
    .map_err(database_error)?;
    Ok(row.map(parse_audit))
}

fn parse_audit(row: AuditRow) -> PersistedAudit {
    let (
        audit_event_id,
        occurred_at,
        actor,
        action_kind,
        resource_type,
        resource_id,
        result,
        reason_code,
        correlation_id,
        redacted_detail_json,
    ) = row;
    PersistedAudit {
        audit_event_id,
        occurred_at,
        actor,
        action_kind: parse_audit_action(&action_kind),
        resource_type: parse_audit_resource(&resource_type),
        resource_id: resource_id.unwrap_or_default(),
        result: parse_audit_result(&result),
        reason_code,
        correlation_id,
        redacted_detail_json,
    }
}

impl PersistedAudit {
    fn as_backing(&self) -> AuditBacking<'_> {
        AuditBacking {
            audit_event_id: &self.audit_event_id,
            occurred_at: &self.occurred_at,
            actor: &self.actor,
            action_kind: self.action_kind,
            resource_type: self.resource_type,
            resource_id: &self.resource_id,
            result: self.result,
            reason_code: self.reason_code.as_deref(),
            correlation_id: &self.correlation_id,
            redacted_detail_json: &self.redacted_detail_json,
        }
    }
}

async fn insert_provisioning_window_audit(
    connection: &mut SqliteConnection,
    change: ProvisioningWindowChange<'_>,
) -> Result<(), GuardedWriteError> {
    let action_kind = match change.state {
        ProvisioningWindowState::Closed => "close_provisioning_window",
        ProvisioningWindowState::Open => "open_provisioning_window",
    };
    query(
        "INSERT INTO audit_events(audit_event_id, occurred_at, actor, action_kind, resource_type, resource_id, result, reason_code, correlation_id, redacted_detail_json) VALUES (?, ?, ?, ?, 'provisioning_window', ?, 'succeeded', ?, ?, ?)",
    )
    .bind(change.audit_event_id)
    .bind(change.occurred_at)
    .bind(change.changed_by)
    .bind(action_kind)
    .bind(PROVISIONING_WINDOW_RESOURCE_ID)
    .bind(change.reason_code)
    .bind(change.correlation_id)
    .bind(change.redacted_detail_json)
    .execute(connection)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn recovery_timestamp(
    connection: &mut SqliteConnection,
) -> Result<String, GuardedWriteError> {
    query_scalar::<_, String>(RECOVERY_TIMESTAMP_SQL)
        .fetch_one(connection)
        .await
        .map_err(database_error)
}

async fn insert_recovery_audit(
    connection: &mut SqliteConnection,
    change: ProvisioningWindowChange<'_>,
) -> Result<(), GuardedWriteError> {
    query(
        "INSERT INTO audit_events(audit_event_id, occurred_at, actor, action_kind, resource_type, resource_id, result, reason_code, correlation_id, redacted_detail_json) VALUES (?, ?, ?, ?, 'provisioning_window', ?, 'succeeded', ?, ?, ?)",
    )
    .bind(change.audit_event_id)
    .bind(change.occurred_at)
    .bind(change.changed_by)
    .bind(RECOVERY_ACTION)
    .bind(PROVISIONING_WINDOW_RESOURCE_ID)
    .bind(change.reason_code)
    .bind(change.correlation_id)
    .bind(change.redacted_detail_json)
    .execute(connection)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn retire_active_credentials(
    connection: &mut SqliteConnection,
    device_pk: &str,
    replacement_token_hash: &[u8],
) -> Result<(), GuardedWriteError> {
    let deleted = query("DELETE FROM device_tokens WHERE device_pk = ? AND token_hash <> ?")
        .bind(device_pk)
        .bind(replacement_token_hash)
        .execute(&mut *connection)
        .await
        .map_err(database_error)?;
    if deleted.rows_affected() != 1 {
        return Err(domain_error(
            DomainCheckError::ReplacementCredentialsRequired,
        ));
    }

    let retired = query(
        "UPDATE gateway_certificates SET status = 'retired' WHERE device_pk = ? AND status = 'active'",
    )
    .bind(device_pk)
    .execute(connection)
    .await
    .map_err(database_error)?;
    if retired.rows_affected() != 1 {
        return Err(domain_error(
            DomainCheckError::ReplacementCredentialsRequired,
        ));
    }
    Ok(())
}

async fn insert_credentials(
    connection: &mut SqliteConnection,
    credentials: EnrollmentCredentials<'_>,
    enrollment: &PersistedEnrollment,
) -> Result<(), GuardedWriteError> {
    query(
        "INSERT INTO device_tokens(device_pk, enrollment_request_id, token_hash) VALUES (?, ?, ?)",
    )
    .bind(credentials.device_pk)
    .bind(&enrollment.enrollment_request_id)
    .bind(credentials.token_hash)
    .execute(&mut *connection)
    .await
    .map_err(database_error)?;
    query(
        "INSERT INTO gateway_certificates(certificate_id, device_pk, enrollment_request_id, serial, spki_sha256, not_after, status) VALUES (?, ?, ?, ?, ?, ?, 'active')",
    )
    .bind(credentials.certificate_id)
    .bind(credentials.device_pk)
    .bind(&enrollment.enrollment_request_id)
    .bind(credentials.certificate_serial)
    .bind(credentials.certificate_spki_sha256)
    .bind(credentials.certificate_not_after)
    .execute(connection)
    .await
    .map_err(database_error)?;
    Ok(())
}

fn validate_resolution(
    actual: EnrollmentResolution,
    expected: EnrollmentResolution,
) -> Result<(), GuardedWriteError> {
    if actual == expected {
        Ok(())
    } else {
        Err(domain_error(
            DomainCheckError::EnrollmentResolutionMismatch { expected, actual },
        ))
    }
}

fn parse_window_state(value: &str) -> Result<ProvisioningWindowState, GuardedWriteError> {
    match value {
        "closed" => Ok(ProvisioningWindowState::Closed),
        "open" => Ok(ProvisioningWindowState::Open),
        _ => Err(domain_error(
            DomainCheckError::PersistedProvisioningWindowInvalid,
        )),
    }
}

fn parse_enrollment_state(value: &str) -> Result<EnrollmentState, GuardedWriteError> {
    match value {
        "pending" => Ok(EnrollmentState::Pending),
        "approved" => Ok(EnrollmentState::Approved),
        "rejected" => Ok(EnrollmentState::Rejected),
        "issued" => Ok(EnrollmentState::Issued),
        "expired" => Ok(EnrollmentState::Expired),
        "conflict" => Ok(EnrollmentState::Conflict),
        _ => Err(domain_error(DomainCheckError::IssuedEnrollmentRequired)),
    }
}

const fn resolution_text(value: EnrollmentResolution) -> &'static str {
    match value {
        EnrollmentResolution::CreateDevice => "create_device",
        EnrollmentResolution::ReplaceDeviceCredentials => "replace_device_credentials",
    }
}

fn parse_resolution(value: &str) -> Option<EnrollmentResolution> {
    match value.as_bytes() {
        b"create_device" => Some(EnrollmentResolution::CreateDevice),
        b"replace_device_credentials" => Some(EnrollmentResolution::ReplaceDeviceCredentials),
        _ => None,
    }
}

fn parse_audit_action(value: &str) -> AuditActionKind {
    match value.as_bytes() {
        b"open_provisioning_window" => AuditActionKind::OpenProvisioningWindow,
        b"close_provisioning_window" => AuditActionKind::CloseProvisioningWindow,
        b"recovery_close_provisioning_window" => AuditActionKind::RecoveryCloseProvisioningWindow,
        b"issue_device_enrollment" => AuditActionKind::IssueDeviceEnrollment,
        b"replace_device_enrollment" => AuditActionKind::ReplaceDeviceEnrollment,
        _ => AuditActionKind::Other,
    }
}

fn parse_audit_resource(value: &str) -> AuditResourceType {
    match value.as_bytes() {
        b"provisioning_window" => AuditResourceType::ProvisioningWindow,
        b"enrollment_request" => AuditResourceType::EnrollmentRequest,
        _ => AuditResourceType::Other,
    }
}

fn parse_audit_result(value: &str) -> AuditResult {
    match value.as_bytes() {
        b"succeeded" => AuditResult::Succeeded,
        b"rejected" => AuditResult::Rejected,
        b"failed" => AuditResult::Failed,
        b"noop" => AuditResult::Noop,
        _ => AuditResult::Other,
    }
}

const fn window_state_text(state: ProvisioningWindowState) -> &'static str {
    match state {
        ProvisioningWindowState::Closed => "closed",
        ProvisioningWindowState::Open => "open",
    }
}

fn domain_error(source: DomainCheckError) -> GuardedWriteError {
    GuardedWriteError::DomainCheck { source }
}

fn database_error(source: sqlx::Error) -> GuardedWriteError {
    GuardedWriteError::Database { source }
}
