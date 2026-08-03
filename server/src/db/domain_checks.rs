//! Pure persistence-policy checks for server-owned domain write paths.
//!
//! SQL owns data shape and declarative cardinality. These functions own mutation policy and
//! receive typed data already read by the caller in its transaction. They do not write data or
//! look up audit events.
//!
//! The guarded persistence API reads that data from `SQLite` before invoking these checks. For an
//! audited mutation, the guarded writer inserts the redacted `audit_events` row and mutation in
//! one nested savepoint; the caller must roll back its outer transaction on any error.
//! [`AuditBacking`] intentionally contains only nonsecret persisted metadata.

use snafu::Snafu;

/// The logical resource identifier for the `provisioning_window` singleton.
pub const PROVISIONING_WINDOW_RESOURCE_ID: &str = "1";

/// An update or deletion attempted against an immutable persisted row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowMutation {
    Update,
    Delete,
}

/// Persisted provisioning-window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisioningWindowState {
    Closed,
    Open,
}

/// The current persisted provisioning-window singleton.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvisioningWindow {
    pub state: ProvisioningWindowState,
    pub revision: i64,
}

/// An audited, operator-controlled mutation of the provisioning-window singleton.
///
/// The guarded writer inserts the audit row represented by this value in its nested savepoint.
/// Its metadata is deliberately redacted and contains no Enrollment credential,
/// CSR, private key, source path, or unredacted error chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvisioningWindowChange<'a> {
    pub state: ProvisioningWindowState,
    pub changed_by: &'a str,
    pub audit_event_id: &'a str,
    pub correlation_id: &'a str,
    pub reason_code: Option<&'a str>,
    pub redacted_detail_json: &'a str,
    pub occurred_at: &'a str,
}

/// Enrollment state relevant to issuance binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentState {
    Pending,
    Approved,
    Rejected,
    Issued,
    Expired,
    Conflict,
}

/// Enrollment resolution determines the required audit action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentResolution {
    CreateDevice,
    ReplaceDeviceCredentials,
}

/// Gateway-certificate lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayCertificateStatus {
    Active,
    Revoked,
    Expired,
    Retired,
}

/// Audited action kinds used by these persistence checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditActionKind {
    OpenProvisioningWindow,
    CloseProvisioningWindow,
    RecoveryCloseProvisioningWindow,
    IssueDeviceEnrollment,
    ReplaceDeviceEnrollment,
    Other,
}

/// Audited resource kinds used by these persistence checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditResourceType {
    ProvisioningWindow,
    EnrollmentRequest,
    Other,
}

/// Audit outcomes relevant to mutation backing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditResult {
    Succeeded,
    Rejected,
    Failed,
    Noop,
    Other,
}

/// Nonsecret metadata for an audit row already inserted by the caller's transaction.
///
/// `redacted_detail_json` is the database-validated redacted evidence object, not raw input or
/// secret material. The fields here are the minimum needed to bind an audited mutation exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditBacking<'a> {
    pub audit_event_id: &'a str,
    pub occurred_at: &'a str,
    pub actor: &'a str,
    pub action_kind: AuditActionKind,
    pub resource_type: AuditResourceType,
    pub resource_id: &'a str,
    pub result: AuditResult,
    pub reason_code: Option<&'a str>,
    pub correlation_id: &'a str,
    pub redacted_detail_json: &'a str,
}

/// Identity columns that an enrollment-request update may not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnrollmentRequestIdentity<'a> {
    pub enrollment_request_id: &'a str,
    pub machine_hardware_id: &'a str,
    pub hardware_identity_quality: &'a str,
    pub gateway_csr_der: &'a [u8],
    pub gateway_spki_sha256: &'a [u8],
    pub client_version: &'a str,
    pub protocol_version: i64,
    pub source_ip: &'a str,
    pub created_at: &'a str,
}

/// Identity columns that a gateway-certificate update may not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayCertificateIdentity<'a> {
    pub certificate_id: &'a str,
    pub device_pk: &'a str,
    pub enrollment_request_id: &'a str,
    pub serial: &'a str,
    pub spki_sha256: &'a [u8],
    pub not_after: &'a str,
}

/// Issued-enrollment data read by a credential-issuance write path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnrollmentIssuanceBinding<'a> {
    pub enrollment_request_id: &'a str,
    pub state: EnrollmentState,
    pub resolved_device_pk: Option<&'a str>,
    pub gateway_spki_sha256: &'a [u8],
}

/// The issuance operation currently gated by provisioning-window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowGatedOperation {
    EnrollmentInsert,
    EnrollmentIssuance,
    DeviceTokenInsert,
    GatewayCertificateInsert,
}

/// Typed persistence-policy failures. Values that could contain secret material are never stored.
#[derive(Debug, Snafu, PartialEq, Eq)]
pub enum DomainCheckError {
    #[snafu(display("site identity already exists"))]
    SiteIdentityAlreadyExists,

    #[snafu(display("site identity does not permit {mutation:?}"))]
    SiteIdentityImmutable { mutation: RowMutation },

    #[snafu(display("seat-code changes must be modeled as a removed Seat and an added Seat"))]
    SeatCodeRenameRequiresReplacement,

    #[snafu(display("machine hardware identity cannot change"))]
    MachineHardwareIdImmutable,

    #[snafu(display("enrollment-request identity cannot change"))]
    EnrollmentRequestIdentityImmutable,

    #[snafu(display("an issued enrollment cannot be updated"))]
    IssuedEnrollmentImmutable,

    #[snafu(display("device-token rows cannot be updated"))]
    DeviceTokenImmutable,

    #[snafu(display("gateway-certificate identity cannot change"))]
    GatewayCertificateIdentityImmutable,

    #[snafu(display(
        "gateway-certificate status transition from {current:?} to {proposed:?} is invalid"
    ))]
    GatewayCertificateStatusTransitionInvalid {
        current: GatewayCertificateStatus,
        proposed: GatewayCertificateStatus,
    },

    #[snafu(display("provisioning-window revision number overflow"))]
    ProvisioningWindowRevisionOverflow,

    #[snafu(display("provisioning-window transition must toggle state"))]
    ProvisioningWindowStateUnchanged,

    #[snafu(display("a recovery provisioning-window transition must close the window"))]
    RecoveryWindowTransitionMustClose,

    #[snafu(display("the persisted provisioning-window singleton is missing or invalid"))]
    PersistedProvisioningWindowInvalid,

    #[snafu(display("required audit backing is missing"))]
    AuditBackingMissing,

    #[snafu(display("audit backing does not match the proposed domain mutation"))]
    AuditBackingMismatch,

    #[snafu(display("the provisioning window is closed for {operation:?}"))]
    ProvisioningWindowClosed { operation: WindowGatedOperation },

    #[snafu(display("credential issuance requires the referenced approved enrollment"))]
    ApprovedEnrollmentRequired,

    #[snafu(display("credential insertion requires the referenced issued enrollment"))]
    IssuedEnrollmentRequired,

    #[snafu(display("issued enrollment is bound to a different device"))]
    EnrollmentDeviceMismatch,

    #[snafu(display("enrollment machine identity does not match the resolved device"))]
    EnrollmentMachineHardwareIdMismatch,

    #[snafu(display("credential issuance requires an enrolled device"))]
    EnrollmentDeviceNotEnrolled,

    #[snafu(display("enrollment resolution mismatch: expected {expected:?}, got {actual:?}"))]
    EnrollmentResolutionMismatch {
        expected: EnrollmentResolution,
        actual: EnrollmentResolution,
    },

    #[snafu(display("credential replacement requires existing active credentials"))]
    ReplacementCredentialsRequired,

    #[snafu(display("issued enrollment Gateway SPKI does not match the certificate"))]
    EnrollmentGatewaySpkiMismatch,
}

/// Enforces insert-once semantics for `site_identity`.
///
/// # Errors
///
/// Returns [`DomainCheckError::SiteIdentityAlreadyExists`] when the singleton row exists.
pub const fn check_site_identity_insert(existing: bool) -> Result<(), DomainCheckError> {
    if existing {
        Err(DomainCheckError::SiteIdentityAlreadyExists)
    } else {
        Ok(())
    }
}

/// Rejects every update or deletion of `site_identity`.
///
/// # Errors
///
/// Always returns [`DomainCheckError::SiteIdentityImmutable`].
pub const fn check_site_identity_mutation(mutation: RowMutation) -> Result<(), DomainCheckError> {
    Err(DomainCheckError::SiteIdentityImmutable { mutation })
}

/// Requires a Seat-code rename to be represented as a removed Seat and an added Seat.
///
/// # Errors
///
/// Returns [`DomainCheckError::SeatCodeRenameRequiresReplacement`] when the code changes.
pub fn check_seat_code_unchanged(current: &str, proposed: &str) -> Result<(), DomainCheckError> {
    if current == proposed {
        Ok(())
    } else {
        Err(DomainCheckError::SeatCodeRenameRequiresReplacement)
    }
}

/// Rejects changes to a device's machine hardware identity.
///
/// # Errors
///
/// Returns [`DomainCheckError::MachineHardwareIdImmutable`] when the identity changes.
pub fn check_machine_hardware_id_unchanged(
    current: &str,
    proposed: &str,
) -> Result<(), DomainCheckError> {
    if current == proposed {
        Ok(())
    } else {
        Err(DomainCheckError::MachineHardwareIdImmutable)
    }
}

/// Rejects changes to any enrollment-request identity column.
///
/// # Errors
///
/// Returns [`DomainCheckError::EnrollmentRequestIdentityImmutable`] on any difference.
pub fn check_enrollment_request_identity_unchanged(
    current: EnrollmentRequestIdentity<'_>,
    proposed: EnrollmentRequestIdentity<'_>,
) -> Result<(), DomainCheckError> {
    if current == proposed {
        Ok(())
    } else {
        Err(DomainCheckError::EnrollmentRequestIdentityImmutable)
    }
}

/// Rejects every update after an enrollment reaches `issued`.
///
/// # Errors
///
/// Returns [`DomainCheckError::IssuedEnrollmentImmutable`] for an issued current row.
pub const fn check_enrollment_request_update(
    current_state: EnrollmentState,
) -> Result<(), DomainCheckError> {
    if matches!(current_state, EnrollmentState::Issued) {
        Err(DomainCheckError::IssuedEnrollmentImmutable)
    } else {
        Ok(())
    }
}

/// Rejects every device-token update. Replacement is delete-then-insert in one transaction.
///
/// # Errors
///
/// Always returns [`DomainCheckError::DeviceTokenImmutable`].
pub const fn check_device_token_update() -> Result<(), DomainCheckError> {
    Err(DomainCheckError::DeviceTokenImmutable)
}

/// Rejects changes to any gateway-certificate identity column.
///
/// # Errors
///
/// Returns [`DomainCheckError::GatewayCertificateIdentityImmutable`] on any difference.
pub fn check_gateway_certificate_identity_unchanged(
    current: GatewayCertificateIdentity<'_>,
    proposed: GatewayCertificateIdentity<'_>,
) -> Result<(), DomainCheckError> {
    if current == proposed {
        Ok(())
    } else {
        Err(DomainCheckError::GatewayCertificateIdentityImmutable)
    }
}

/// Allows only `active` to `revoked`, `expired`, or `retired` certificate transitions.
///
/// # Errors
///
/// Returns [`DomainCheckError::GatewayCertificateStatusTransitionInvalid`] otherwise.
pub const fn check_gateway_certificate_status_transition(
    current: GatewayCertificateStatus,
    proposed: GatewayCertificateStatus,
) -> Result<(), DomainCheckError> {
    match (current, proposed) {
        (
            GatewayCertificateStatus::Active,
            GatewayCertificateStatus::Revoked
            | GatewayCertificateStatus::Expired
            | GatewayCertificateStatus::Retired,
        ) => Ok(()),
        _ => Err(DomainCheckError::GatewayCertificateStatusTransitionInvalid { current, proposed }),
    }
}

/// Validates a state-toggling, audited provisioning-window singleton transition.
///
/// The guarded writer reads the singleton, inserts the audit represented by `change`, and performs
/// a state-and-revision compare-and-swap in one nested savepoint before the caller commits.
///
/// # Errors
///
/// Returns a typed toggle, overflow, or audit-backing error.
pub fn check_provisioning_window_transition(
    current: ProvisioningWindow,
    change: ProvisioningWindowChange<'_>,
    audit: Option<AuditBacking<'_>>,
) -> Result<(), DomainCheckError> {
    let expected_action = match change.state {
        ProvisioningWindowState::Closed => AuditActionKind::CloseProvisioningWindow,
        ProvisioningWindowState::Open => AuditActionKind::OpenProvisioningWindow,
    };
    check_provisioning_window_transition_for_action(current, change, audit, expected_action)
}

/// Validates the audited close-only transition used to fail closed during recovery.
///
/// # Errors
///
/// Returns a typed recovery-state, toggle, overflow, or audit-backing error.
pub fn check_recovery_provisioning_window_transition(
    current: ProvisioningWindow,
    change: ProvisioningWindowChange<'_>,
    audit: Option<AuditBacking<'_>>,
) -> Result<(), DomainCheckError> {
    if change.state != ProvisioningWindowState::Closed {
        return Err(DomainCheckError::RecoveryWindowTransitionMustClose);
    }
    check_provisioning_window_transition_for_action(
        current,
        change,
        audit,
        AuditActionKind::RecoveryCloseProvisioningWindow,
    )
}

fn check_provisioning_window_transition_for_action(
    current: ProvisioningWindow,
    change: ProvisioningWindowChange<'_>,
    audit: Option<AuditBacking<'_>>,
    expected_action: AuditActionKind,
) -> Result<(), DomainCheckError> {
    if current.revision < 0 {
        return Err(DomainCheckError::PersistedProvisioningWindowInvalid);
    }
    if current.revision.checked_add(1).is_none() {
        return Err(DomainCheckError::ProvisioningWindowRevisionOverflow);
    }
    if current.state == change.state {
        return Err(DomainCheckError::ProvisioningWindowStateUnchanged);
    }
    let expected = ExpectedAuditBacking {
        audit_event_id: change.audit_event_id,
        occurred_at: Some(change.occurred_at),
        actor: Some(change.changed_by),
        action_kind: expected_action,
        resource_type: AuditResourceType::ProvisioningWindow,
        resource_id: PROVISIONING_WINDOW_RESOURCE_ID,
        correlation_id: Some(change.correlation_id),
        reason_code: ExpectedOptional::Exact(change.reason_code),
        redacted_detail_json: Some(change.redacted_detail_json),
    };
    check_audit_backing(audit, expected)
}

/// Rejects enrollment insertion while the persisted provisioning window is closed.
///
/// # Errors
///
/// Returns [`DomainCheckError::ProvisioningWindowClosed`] unless `window` is open.
pub const fn check_enrollment_insert(
    window: ProvisioningWindowState,
) -> Result<(), DomainCheckError> {
    check_window_open(window, WindowGatedOperation::EnrollmentInsert)
}

/// Rejects Enrollment issuance while the persisted provisioning window is closed.
///
/// # Errors
///
/// Returns [`DomainCheckError::ProvisioningWindowClosed`] unless `window` is open.
pub const fn check_enrollment_issuance_window(
    window: ProvisioningWindowState,
) -> Result<(), DomainCheckError> {
    check_window_open(window, WindowGatedOperation::EnrollmentIssuance)
}

/// Validates window gating and exact core audit metadata for enrollment issuance.
///
/// The guarded writer inserts the represented audit row and transitions the Enrollment to issued
/// in its nested savepoint. The audit row contains redacted metadata only; this check accepts no
/// token, private key, raw upload, CSR, or error-chain value.
///
/// # Errors
///
/// Returns a typed window or audit-backing error.
pub fn check_enrollment_issuance(
    window: ProvisioningWindowState,
    enrollment_request_id: &str,
    issuance_audit_event_id: &str,
    resolution: EnrollmentResolution,
    issuing_actor: &str,
    audit: Option<AuditBacking<'_>>,
) -> Result<(), DomainCheckError> {
    check_enrollment_issuance_window(window)?;
    let action_kind = match resolution {
        EnrollmentResolution::CreateDevice => AuditActionKind::IssueDeviceEnrollment,
        EnrollmentResolution::ReplaceDeviceCredentials => AuditActionKind::ReplaceDeviceEnrollment,
    };
    let expected = ExpectedAuditBacking {
        audit_event_id: issuance_audit_event_id,
        occurred_at: None,
        actor: Some(issuing_actor),
        action_kind,
        resource_type: AuditResourceType::EnrollmentRequest,
        resource_id: enrollment_request_id,
        correlation_id: None,
        reason_code: ExpectedOptional::Any,
        redacted_detail_json: None,
    };
    check_audit_backing(audit, expected)
}

/// Validates window gating and issued-enrollment device binding for a new device token.
///
/// # Errors
///
/// Returns a typed window, issued-enrollment, or device-binding error.
pub fn check_device_token_insert(
    window: ProvisioningWindowState,
    device_pk: &str,
    enrollment_request_id: &str,
    enrollment: Option<EnrollmentIssuanceBinding<'_>>,
) -> Result<(), DomainCheckError> {
    check_window_open(window, WindowGatedOperation::DeviceTokenInsert)?;
    let enrollment = require_issued_enrollment(enrollment_request_id, enrollment)?;
    if enrollment.resolved_device_pk != Some(device_pk) {
        return Err(DomainCheckError::EnrollmentDeviceMismatch);
    }
    Ok(())
}

/// Validates window gating and issued-enrollment device/SPKI binding for a new certificate.
///
/// # Errors
///
/// Returns a typed window, issued-enrollment, device-binding, or SPKI-binding error.
pub fn check_gateway_certificate_insert(
    window: ProvisioningWindowState,
    device_pk: &str,
    enrollment_request_id: &str,
    certificate_spki_sha256: &[u8],
    enrollment: Option<EnrollmentIssuanceBinding<'_>>,
) -> Result<(), DomainCheckError> {
    check_window_open(window, WindowGatedOperation::GatewayCertificateInsert)?;
    let enrollment = require_issued_enrollment(enrollment_request_id, enrollment)?;
    if enrollment.resolved_device_pk != Some(device_pk) {
        return Err(DomainCheckError::EnrollmentDeviceMismatch);
    }
    if enrollment.gateway_spki_sha256 != certificate_spki_sha256 {
        return Err(DomainCheckError::EnrollmentGatewaySpkiMismatch);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum ExpectedOptional<T> {
    Any,
    Exact(Option<T>),
}

impl<T: PartialEq> ExpectedOptional<T> {
    fn matches(&self, actual: Option<&T>) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(expected) => expected.as_ref() == actual,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ExpectedAuditBacking<'a> {
    audit_event_id: &'a str,
    occurred_at: Option<&'a str>,
    actor: Option<&'a str>,
    action_kind: AuditActionKind,
    resource_type: AuditResourceType,
    resource_id: &'a str,
    correlation_id: Option<&'a str>,
    reason_code: ExpectedOptional<&'a str>,
    redacted_detail_json: Option<&'a str>,
}

fn check_audit_backing(
    audit: Option<AuditBacking<'_>>,
    expected: ExpectedAuditBacking<'_>,
) -> Result<(), DomainCheckError> {
    let Some(audit) = audit else {
        return Err(DomainCheckError::AuditBackingMissing);
    };
    let actor_matches = expected.actor.is_none_or(|actor| actor == audit.actor);
    let occurred_at_matches = expected
        .occurred_at
        .is_none_or(|occurred_at| occurred_at == audit.occurred_at);
    let correlation_matches = expected
        .correlation_id
        .is_none_or(|correlation_id| correlation_id == audit.correlation_id);
    let reason_matches = expected.reason_code.matches(audit.reason_code.as_ref());
    let detail_matches = expected
        .redacted_detail_json
        .is_none_or(|detail| detail == audit.redacted_detail_json);
    if audit.audit_event_id != expected.audit_event_id
        || !occurred_at_matches
        || !actor_matches
        || audit.action_kind != expected.action_kind
        || audit.resource_type != expected.resource_type
        || audit.resource_id != expected.resource_id
        || audit.result != AuditResult::Succeeded
        || !correlation_matches
        || !reason_matches
        || !detail_matches
    {
        return Err(DomainCheckError::AuditBackingMismatch);
    }
    Ok(())
}

const fn check_window_open(
    window: ProvisioningWindowState,
    operation: WindowGatedOperation,
) -> Result<(), DomainCheckError> {
    if matches!(window, ProvisioningWindowState::Open) {
        Ok(())
    } else {
        Err(DomainCheckError::ProvisioningWindowClosed { operation })
    }
}

fn require_issued_enrollment<'a>(
    enrollment_request_id: &str,
    enrollment: Option<EnrollmentIssuanceBinding<'a>>,
) -> Result<EnrollmentIssuanceBinding<'a>, DomainCheckError> {
    let Some(enrollment) = enrollment else {
        return Err(DomainCheckError::IssuedEnrollmentRequired);
    };
    if enrollment.enrollment_request_id != enrollment_request_id
        || enrollment.state != EnrollmentState::Issued
    {
        return Err(DomainCheckError::IssuedEnrollmentRequired);
    }
    Ok(enrollment)
}

#[cfg(test)]
mod tests {
    use sqlx::{Sqlite, SqlitePool, Transaction, query, query_as, sqlite::SqlitePoolOptions};

    use super::*;
    use crate::db::guarded::{
        GuardedWriteError, change_provisioning_window, close_provisioning_window_for_recovery,
        compare_and_swap_provisioning_window,
    };

    const OPERATOR: &str = "operator-1";
    const OCCURRED_AT: &str = "2026-08-03T00:00:00Z";

    struct AuditFixture<'a> {
        audit_event_id: &'a str,
        occurred_at: &'a str,
        actor: &'a str,
        action_kind: &'a str,
        resource_type: &'a str,
        resource_id: &'a str,
        result: &'a str,
        reason_code: Option<&'a str>,
        correlation_id: &'a str,
        redacted_detail_json: &'a str,
    }

    async fn migrated_pool() -> SqlitePool {
        let Ok(pool) = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
        else {
            panic!("in-memory SQLite must connect");
        };
        if let Err(error) = crate::db::MIGRATOR.run(&pool).await {
            panic!("real embedded migration must execute: {error}");
        }
        pool
    }

    fn window_state(value: &str) -> ProvisioningWindowState {
        match value {
            "closed" => ProvisioningWindowState::Closed,
            "open" => ProvisioningWindowState::Open,
            _ => panic!("schema CHECK must keep provisioning-window state closed or open"),
        }
    }

    fn enrollment_state(value: &str) -> EnrollmentState {
        match value {
            "pending" => EnrollmentState::Pending,
            "approved" => EnrollmentState::Approved,
            "rejected" => EnrollmentState::Rejected,
            "issued" => EnrollmentState::Issued,
            "expired" => EnrollmentState::Expired,
            "conflict" => EnrollmentState::Conflict,
            _ => panic!("schema CHECK must keep enrollment state in its closed enum"),
        }
    }

    async fn current_window(pool: &SqlitePool) -> ProvisioningWindow {
        let Ok((state, revision)) = query_as::<_, (String, i64)>(
            "SELECT state, revision FROM provisioning_window WHERE singleton = 1",
        )
        .fetch_one(pool)
        .await
        else {
            panic!("provisioning-window singleton must be queryable");
        };
        ProvisioningWindow {
            state: window_state(&state),
            revision,
        }
    }

    async fn insert_audit(transaction: &mut Transaction<'_, Sqlite>, audit: AuditFixture<'_>) {
        if let Err(error) = query(
            "INSERT INTO audit_events(audit_event_id, occurred_at, actor, action_kind, resource_type, resource_id, result, reason_code, correlation_id, redacted_detail_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(audit.audit_event_id)
        .bind(audit.occurred_at)
        .bind(audit.actor)
        .bind(audit.action_kind)
        .bind(audit.resource_type)
        .bind(audit.resource_id)
        .bind(audit.result)
        .bind(audit.reason_code)
        .bind(audit.correlation_id)
        .bind(audit.redacted_detail_json)
        .execute(&mut **transaction)
        .await
        {
            panic!("audit fixture must satisfy the authoritative migration: {error}");
        }
    }

    fn backing<'a>(
        audit_event_id: &'a str,
        action_kind: AuditActionKind,
        resource_type: AuditResourceType,
        resource_id: &'a str,
        redacted_detail_json: &'a str,
    ) -> AuditBacking<'a> {
        AuditBacking {
            audit_event_id,
            occurred_at: OCCURRED_AT,
            actor: OPERATOR,
            action_kind,
            resource_type,
            resource_id,
            result: AuditResult::Succeeded,
            reason_code: Some("operator_requested"),
            correlation_id: "correlation-1",
            redacted_detail_json,
        }
    }

    async fn change_window(
        pool: &SqlitePool,
        state: ProvisioningWindowState,
        audit_event_id: &str,
        correlation_id: &str,
        occurred_at: &str,
        redacted_detail_json: &str,
    ) -> ProvisioningWindow {
        let Ok(mut transaction) = pool.begin().await else {
            panic!("window transition transaction must begin");
        };
        let change = ProvisioningWindowChange {
            state,
            changed_by: OPERATOR,
            audit_event_id,
            correlation_id,
            reason_code: Some("operator_requested"),
            redacted_detail_json,
            occurred_at,
        };
        let Ok(window) = change_provisioning_window(&mut transaction, change).await else {
            panic!("valid provisioning-window transition must pass");
        };
        if let Err(error) = transaction.commit().await {
            panic!("audit and singleton transition must commit atomically: {error}");
        }
        window
    }

    async fn seed_issued_enrollments(pool: &SqlitePool) {
        let statements = [
            "INSERT INTO devices(device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES ('device-1', 'machine-1', 'strong', 'enrolled')",
            "INSERT INTO audit_events(audit_event_id, occurred_at, actor, action_kind, resource_type, resource_id, result, correlation_id, redacted_detail_json) VALUES ('audit-enrollment-1', '2026-08-03T00:01:00Z', 'operator-1', 'issue_device_enrollment', 'enrollment_request', 'enrollment-1', 'succeeded', 'correlation-enrollment-1', '{}')",
            "INSERT INTO audit_events(audit_event_id, occurred_at, actor, action_kind, resource_type, resource_id, result, correlation_id, redacted_detail_json) VALUES ('audit-enrollment-2', '2026-08-03T00:02:00Z', 'operator-1', 'replace_device_enrollment', 'enrollment_request', 'enrollment-2', 'succeeded', 'correlation-enrollment-2', '{}')",
            "INSERT INTO enrollment_requests(enrollment_request_id, machine_hardware_id, hardware_identity_quality, gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, source_ip, state, resolution, resolved_device_pk, issuance_audit_event_id, created_at) VALUES ('enrollment-1', 'machine-1', 'strong', x'01', x'1111111111111111111111111111111111111111111111111111111111111111', 'test-client', 1, '192.0.2.1', 'issued', 'create_device', 'device-1', 'audit-enrollment-1', '2026-08-03T00:01:00Z')",
            "INSERT INTO enrollment_requests(enrollment_request_id, machine_hardware_id, hardware_identity_quality, gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, source_ip, state, resolution, resolved_device_pk, issuance_audit_event_id, created_at) VALUES ('enrollment-2', 'machine-1', 'strong', x'02', x'2222222222222222222222222222222222222222222222222222222222222222', 'test-client', 1, '192.0.2.1', 'issued', 'replace_device_credentials', 'device-1', 'audit-enrollment-2', '2026-08-03T00:02:00Z')",
        ];
        for statement in statements {
            if let Err(error) = query(statement).execute(pool).await {
                panic!("issued-enrollment fixture must insert: {error}");
            }
        }
    }

    async fn read_enrollment_binding<'a>(
        pool: &SqlitePool,
        enrollment_request_id: &'a str,
    ) -> (Option<String>, Vec<u8>, EnrollmentState, &'a str) {
        let Ok((state, resolved_device_pk, spki)) =
            query_as::<_, (String, Option<String>, Vec<u8>)>(
                "SELECT state, resolved_device_pk, gateway_spki_sha256 FROM enrollment_requests WHERE enrollment_request_id = ?",
            )
            .bind(enrollment_request_id)
            .fetch_one(pool)
            .await
        else {
            panic!("enrollment binding must be queryable");
        };
        (
            resolved_device_pk,
            spki,
            enrollment_state(&state),
            enrollment_request_id,
        )
    }

    #[tokio::test]
    async fn singleton_window_toggles_once_per_audited_transition() {
        let pool = migrated_pool().await;
        assert_eq!(
            current_window(&pool).await,
            ProvisioningWindow {
                state: ProvisioningWindowState::Closed,
                revision: 0,
            }
        );

        let opened = change_window(
            &pool,
            ProvisioningWindowState::Open,
            "audit-window-open-1",
            "correlation-window-open-1",
            "2026-08-03T00:00:00Z",
            r#"{"from_state":"closed","to_state":"open"}"#,
        )
        .await;
        assert_eq!(
            opened,
            ProvisioningWindow {
                state: ProvisioningWindowState::Open,
                revision: 1,
            }
        );

        let closed = change_window(
            &pool,
            ProvisioningWindowState::Closed,
            "audit-window-close-2",
            "correlation-window-close-2",
            "2026-08-03T00:01:00Z",
            r#"{"from_state":"open","to_state":"closed"}"#,
        )
        .await;
        assert_eq!(
            closed,
            ProvisioningWindow {
                state: ProvisioningWindowState::Closed,
                revision: 2,
            }
        );

        let Ok((singleton_count, state, revision, last_audit_event_id)) =
            query_as::<_, (i64, String, i64, Option<String>)>(
                "SELECT (SELECT COUNT(*) FROM provisioning_window), state, revision, last_audit_event_id FROM provisioning_window WHERE singleton = 1",
            )
            .fetch_one(&pool)
            .await
        else {
            panic!("singleton state must remain queryable");
        };
        assert_eq!(singleton_count, 1);
        assert_eq!(state, "closed");
        assert_eq!(revision, 2);
        assert_eq!(last_audit_event_id.as_deref(), Some("audit-window-close-2"));
    }

    #[test]
    fn window_transition_requires_toggle_and_exact_redacted_audit_metadata() {
        let current = ProvisioningWindow {
            state: ProvisioningWindowState::Closed,
            revision: 0,
        };
        let no_op = ProvisioningWindowChange {
            state: ProvisioningWindowState::Closed,
            changed_by: OPERATOR,
            audit_event_id: "audit-noop",
            correlation_id: "correlation-noop",
            reason_code: Some("operator_requested"),
            redacted_detail_json: "{}",
            occurred_at: OCCURRED_AT,
        };
        assert_eq!(
            check_provisioning_window_transition(current, no_op, None),
            Err(DomainCheckError::ProvisioningWindowStateUnchanged)
        );

        let change = ProvisioningWindowChange {
            state: ProvisioningWindowState::Open,
            changed_by: OPERATOR,
            audit_event_id: "audit-open",
            correlation_id: "correlation-open",
            reason_code: Some("operator_requested"),
            redacted_detail_json: r#"{"from_state":"closed","to_state":"open"}"#,
            occurred_at: OCCURRED_AT,
        };
        let mismatched = AuditBacking {
            correlation_id: "wrong-correlation",
            redacted_detail_json: "{}",
            ..backing(
                "audit-open",
                AuditActionKind::OpenProvisioningWindow,
                AuditResourceType::ProvisioningWindow,
                PROVISIONING_WINDOW_RESOURCE_ID,
                change.redacted_detail_json,
            )
        };
        assert_eq!(
            check_provisioning_window_transition(current, change, Some(mismatched)),
            Err(DomainCheckError::AuditBackingMismatch)
        );
    }

    #[test]
    fn window_transition_rejects_revision_overflow() {
        let current = ProvisioningWindow {
            state: ProvisioningWindowState::Closed,
            revision: i64::MAX,
        };
        let change = ProvisioningWindowChange {
            state: ProvisioningWindowState::Open,
            changed_by: OPERATOR,
            audit_event_id: "audit-overflow",
            correlation_id: "correlation-overflow",
            reason_code: Some("operator_requested"),
            redacted_detail_json: "{}",
            occurred_at: OCCURRED_AT,
        };
        assert_eq!(
            check_provisioning_window_transition(current, change, None),
            Err(DomainCheckError::ProvisioningWindowRevisionOverflow)
        );
    }

    #[tokio::test]
    async fn singleton_compare_and_swap_rejects_a_stale_state_or_revision() {
        let pool = migrated_pool().await;
        let Ok(mut transaction) = pool.begin().await else {
            panic!("CAS test transaction must begin");
        };
        insert_audit(
            &mut transaction,
            AuditFixture {
                audit_event_id: "audit-stale-cas",
                occurred_at: OCCURRED_AT,
                actor: OPERATOR,
                action_kind: "close_provisioning_window",
                resource_type: "provisioning_window",
                resource_id: PROVISIONING_WINDOW_RESOURCE_ID,
                result: "succeeded",
                reason_code: Some("operator_requested"),
                correlation_id: "correlation-stale-cas",
                redacted_detail_json: "{}",
            },
        )
        .await;
        let change = ProvisioningWindowChange {
            state: ProvisioningWindowState::Closed,
            changed_by: OPERATOR,
            audit_event_id: "audit-stale-cas",
            correlation_id: "correlation-stale-cas",
            reason_code: Some("operator_requested"),
            redacted_detail_json: "{}",
            occurred_at: OCCURRED_AT,
        };
        let result = compare_and_swap_provisioning_window(
            &mut transaction,
            ProvisioningWindow {
                state: ProvisioningWindowState::Open,
                revision: 0,
            },
            ProvisioningWindow {
                state: ProvisioningWindowState::Closed,
                revision: 1,
            },
            change,
        )
        .await;
        assert!(matches!(
            result,
            Err(GuardedWriteError::ProvisioningWindowCompareAndSwap { affected_rows: 0 })
        ));
        if let Err(error) = transaction.rollback().await {
            panic!("stale-CAS fixture must roll back: {error}");
        }
    }

    #[tokio::test]
    async fn recovery_closes_open_singleton_once_and_never_reopens_it() {
        let pool = migrated_pool().await;
        change_window(
            &pool,
            ProvisioningWindowState::Open,
            "audit-window-open-for-recovery",
            "correlation-window-open-for-recovery",
            "2026-08-03T00:00:00Z",
            r#"{"from_state":"closed","to_state":"open"}"#,
        )
        .await;

        let Ok(mut first_recovery) = pool.begin().await else {
            panic!("recovery transaction must begin");
        };
        let Ok(closed) = close_provisioning_window_for_recovery(&mut first_recovery).await else {
            panic!("open provisioning window must close during recovery");
        };
        assert!(closed);
        if let Err(error) = first_recovery.commit().await {
            panic!("recovery close must commit atomically: {error}");
        }

        assert_eq!(
            current_window(&pool).await,
            ProvisioningWindow {
                state: ProvisioningWindowState::Closed,
                revision: 2,
            }
        );
        let Ok((audit_count, actor, action, reason_code, detail)) =
            query_as::<_, (i64, String, String, Option<String>, String)>(
                "SELECT (SELECT COUNT(*) FROM audit_events WHERE actor = 'system:recovery'), actor, action_kind, reason_code, redacted_detail_json FROM audit_events WHERE actor = 'system:recovery'",
            )
            .fetch_one(&pool)
            .await
        else {
            panic!("recovery audit must be queryable");
        };
        assert_eq!(audit_count, 1);
        assert_eq!(actor, "system:recovery");
        assert_eq!(action, "recovery_close_provisioning_window");
        assert_eq!(reason_code.as_deref(), Some("recovery_fail_closed"));
        assert_eq!(detail, r#"{"from_state":"open","to_state":"closed"}"#);

        let Ok(mut second_recovery) = pool.begin().await else {
            panic!("second recovery transaction must begin");
        };
        let Ok(closed_again) = close_provisioning_window_for_recovery(&mut second_recovery).await
        else {
            panic!("closed provisioning window recovery must be a no-op");
        };
        assert!(!closed_again);
        if let Err(error) = second_recovery.commit().await {
            panic!("closed recovery no-op transaction must commit: {error}");
        }
        let Ok(audit_count) = query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM audit_events WHERE actor = 'system:recovery'",
        )
        .fetch_one(&pool)
        .await
        else {
            panic!("recovery audit count must remain queryable");
        };
        assert_eq!(audit_count.0, 1);
    }

    #[test]
    fn recovery_transition_is_close_only() {
        let current = ProvisioningWindow {
            state: ProvisioningWindowState::Open,
            revision: 1,
        };
        let reopening_change = ProvisioningWindowChange {
            state: ProvisioningWindowState::Open,
            changed_by: "system:recovery",
            audit_event_id: "audit-recovery-open",
            correlation_id: "correlation-recovery-open",
            reason_code: Some("recovery_fail_closed"),
            redacted_detail_json: "{}",
            occurred_at: OCCURRED_AT,
        };
        assert_eq!(
            check_recovery_provisioning_window_transition(current, reopening_change, None),
            Err(DomainCheckError::RecoveryWindowTransitionMustClose)
        );
    }

    #[tokio::test]
    async fn closed_window_rejects_every_issuance_gate() {
        let pool = migrated_pool().await;
        let window = current_window(&pool).await;
        let spki = [1_u8; 32];
        let binding = EnrollmentIssuanceBinding {
            enrollment_request_id: "enrollment-refused",
            state: EnrollmentState::Issued,
            resolved_device_pk: Some("device-refused"),
            gateway_spki_sha256: &spki,
        };
        assert_eq!(
            check_enrollment_insert(window.state),
            Err(DomainCheckError::ProvisioningWindowClosed {
                operation: WindowGatedOperation::EnrollmentInsert,
            })
        );
        assert_eq!(
            check_enrollment_issuance(
                window.state,
                "enrollment-refused",
                "audit-refused",
                EnrollmentResolution::CreateDevice,
                OPERATOR,
                None,
            ),
            Err(DomainCheckError::ProvisioningWindowClosed {
                operation: WindowGatedOperation::EnrollmentIssuance,
            })
        );
        assert_eq!(
            check_device_token_insert(
                window.state,
                "device-refused",
                "enrollment-refused",
                Some(binding),
            ),
            Err(DomainCheckError::ProvisioningWindowClosed {
                operation: WindowGatedOperation::DeviceTokenInsert,
            })
        );
        assert_eq!(
            check_gateway_certificate_insert(
                window.state,
                "device-refused",
                "enrollment-refused",
                &spki,
                Some(binding),
            ),
            Err(DomainCheckError::ProvisioningWindowClosed {
                operation: WindowGatedOperation::GatewayCertificateInsert,
            })
        );
    }

    #[test]
    fn open_window_issuance_requires_matching_audit_backing() {
        let audit = AuditBacking {
            audit_event_id: "audit-enrollment-1",
            occurred_at: OCCURRED_AT,
            actor: OPERATOR,
            action_kind: AuditActionKind::IssueDeviceEnrollment,
            resource_type: AuditResourceType::EnrollmentRequest,
            resource_id: "enrollment-1",
            result: AuditResult::Succeeded,
            reason_code: None,
            correlation_id: "correlation-enrollment-1",
            redacted_detail_json: "{}",
        };
        assert!(
            check_enrollment_issuance(
                ProvisioningWindowState::Open,
                "enrollment-1",
                "audit-enrollment-1",
                EnrollmentResolution::CreateDevice,
                OPERATOR,
                Some(audit),
            )
            .is_ok()
        );
        assert_eq!(
            check_enrollment_issuance(
                ProvisioningWindowState::Open,
                "enrollment-1",
                "audit-enrollment-1",
                EnrollmentResolution::CreateDevice,
                OPERATOR,
                Some(AuditBacking {
                    action_kind: AuditActionKind::ReplaceDeviceEnrollment,
                    ..audit
                }),
            ),
            Err(DomainCheckError::AuditBackingMismatch)
        );
    }

    #[tokio::test]
    async fn token_and_certificate_require_matching_open_issued_enrollment() {
        let pool = migrated_pool().await;
        seed_issued_enrollments(&pool).await;
        let (device, spki, state, enrollment_id) =
            read_enrollment_binding(&pool, "enrollment-1").await;
        let binding = EnrollmentIssuanceBinding {
            enrollment_request_id: enrollment_id,
            state,
            resolved_device_pk: device.as_deref(),
            gateway_spki_sha256: &spki,
        };
        assert!(
            check_device_token_insert(
                ProvisioningWindowState::Open,
                "device-1",
                "enrollment-1",
                Some(binding),
            )
            .is_ok()
        );
        assert_eq!(
            check_device_token_insert(
                ProvisioningWindowState::Open,
                "device-other",
                "enrollment-1",
                Some(binding),
            ),
            Err(DomainCheckError::EnrollmentDeviceMismatch)
        );
        assert!(
            check_gateway_certificate_insert(
                ProvisioningWindowState::Open,
                "device-1",
                "enrollment-1",
                &spki,
                Some(binding),
            )
            .is_ok()
        );
        assert_eq!(
            check_gateway_certificate_insert(
                ProvisioningWindowState::Open,
                "device-1",
                "enrollment-1",
                &[0_u8; 32],
                Some(binding),
            ),
            Err(DomainCheckError::EnrollmentGatewaySpkiMismatch)
        );
    }

    #[tokio::test]
    async fn one_active_token_requires_delete_then_insert_replacement() {
        let pool = migrated_pool().await;
        seed_issued_enrollments(&pool).await;
        let first = "INSERT INTO device_tokens(device_pk, enrollment_request_id, token_hash) VALUES ('device-1', 'enrollment-1', x'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')";
        if let Err(error) = query(first).execute(&pool).await {
            panic!("first active token must satisfy declarative constraints: {error}");
        }
        let second = "INSERT INTO device_tokens(device_pk, enrollment_request_id, token_hash) VALUES ('device-1', 'enrollment-2', x'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb')";
        let Err(sqlx::Error::Database(error)) = query(second).execute(&pool).await else {
            panic!("concurrent second active token must violate its unique index");
        };
        assert!(error.is_unique_violation());

        let Ok(mut transaction) = pool.begin().await else {
            panic!("token replacement transaction must begin");
        };
        if let Err(error) = query("DELETE FROM device_tokens WHERE device_pk = 'device-1'")
            .execute(&mut *transaction)
            .await
        {
            panic!("old token deletion must succeed: {error}");
        }
        if let Err(error) = query(second).execute(&mut *transaction).await {
            panic!("new token insert after deletion must succeed: {error}");
        }
        if let Err(error) = transaction.commit().await {
            panic!("delete-then-insert replacement must commit: {error}");
        }
        let Ok((count,)) = query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM device_tokens WHERE device_pk = 'device-1'",
        )
        .fetch_one(&pool)
        .await
        else {
            panic!("active token count must be queryable");
        };
        assert_eq!(count, 1);
    }

    #[test]
    fn gateway_certificate_status_is_one_way() {
        assert!(
            check_gateway_certificate_status_transition(
                GatewayCertificateStatus::Active,
                GatewayCertificateStatus::Revoked,
            )
            .is_ok()
        );
        assert!(
            check_gateway_certificate_status_transition(
                GatewayCertificateStatus::Active,
                GatewayCertificateStatus::Expired,
            )
            .is_ok()
        );
        assert_eq!(
            check_gateway_certificate_status_transition(
                GatewayCertificateStatus::Revoked,
                GatewayCertificateStatus::Active,
            ),
            Err(
                DomainCheckError::GatewayCertificateStatusTransitionInvalid {
                    current: GatewayCertificateStatus::Revoked,
                    proposed: GatewayCertificateStatus::Active,
                }
            )
        );
    }

    #[test]
    fn remaining_site_device_enrollment_and_certificate_invariants_are_explicit() {
        assert!(check_site_identity_insert(false).is_ok());
        assert_eq!(
            check_site_identity_insert(true),
            Err(DomainCheckError::SiteIdentityAlreadyExists)
        );
        assert_eq!(
            check_site_identity_mutation(RowMutation::Update),
            Err(DomainCheckError::SiteIdentityImmutable {
                mutation: RowMutation::Update,
            })
        );
        assert!(check_seat_code_unchanged("A-01", "A-01").is_ok());
        assert_eq!(
            check_seat_code_unchanged("A-01", "A-02"),
            Err(DomainCheckError::SeatCodeRenameRequiresReplacement)
        );
        assert!(check_machine_hardware_id_unchanged("machine-1", "machine-1").is_ok());
        assert_eq!(
            check_machine_hardware_id_unchanged("machine-1", "machine-2"),
            Err(DomainCheckError::MachineHardwareIdImmutable)
        );

        let identity = EnrollmentRequestIdentity {
            enrollment_request_id: "enrollment-1",
            machine_hardware_id: "machine-1",
            hardware_identity_quality: "strong",
            gateway_csr_der: &[1_u8],
            gateway_spki_sha256: &[2_u8; 32],
            client_version: "test-client",
            protocol_version: 1,
            source_ip: "192.0.2.1",
            created_at: OCCURRED_AT,
        };
        assert!(check_enrollment_request_identity_unchanged(identity, identity).is_ok());
        assert_eq!(
            check_enrollment_request_identity_unchanged(
                identity,
                EnrollmentRequestIdentity {
                    source_ip: "192.0.2.2",
                    ..identity
                },
            ),
            Err(DomainCheckError::EnrollmentRequestIdentityImmutable)
        );
        assert_eq!(
            check_enrollment_request_update(EnrollmentState::Issued),
            Err(DomainCheckError::IssuedEnrollmentImmutable)
        );
        assert!(check_enrollment_request_update(EnrollmentState::Approved).is_ok());
        assert_eq!(
            check_device_token_update(),
            Err(DomainCheckError::DeviceTokenImmutable)
        );

        let certificate = GatewayCertificateIdentity {
            certificate_id: "certificate-1",
            device_pk: "device-1",
            enrollment_request_id: "enrollment-1",
            serial: "serial-1",
            spki_sha256: &[2_u8; 32],
            not_after: "2026-09-01T00:00:00Z",
        };
        assert!(check_gateway_certificate_identity_unchanged(certificate, certificate).is_ok());
        assert_eq!(
            check_gateway_certificate_identity_unchanged(
                certificate,
                GatewayCertificateIdentity {
                    serial: "serial-2",
                    ..certificate
                },
            ),
            Err(DomainCheckError::GatewayCertificateIdentityImmutable)
        );
    }
}
