use diesel::{ExpressionMethods, RunQueryDsl, dsl::sql, sql_types::Text};

use crate::{
    application::{
        command::CommandError,
        device::{DeviceError, enrollment::EnrollmentError},
        import::ImportError,
        operator::OperatorError,
        provisioning::ProvisioningError,
    },
    audit::AuditEvent,
    db::{Transaction, schema::audit_events},
};

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    event: &AuditEvent,
) -> Result<(), CommandError> {
    insert_persisted(transaction, event).map_err(CommandError::from)
}

pub(crate) fn insert_operator(
    transaction: &mut Transaction<'_>,
    event: &AuditEvent,
) -> Result<(), OperatorError> {
    insert_persisted(transaction, event).map_err(OperatorError::from)
}

pub(crate) fn insert_device(
    transaction: &mut Transaction<'_>,
    event: &AuditEvent,
) -> Result<(), DeviceError> {
    insert_persisted(transaction, event).map_err(DeviceError::from)
}

pub(crate) fn insert_provisioning(
    transaction: &mut Transaction<'_>,
    event: &AuditEvent,
) -> Result<(), ProvisioningError> {
    insert_persisted(transaction, event).map_err(ProvisioningError::from)
}

pub(crate) fn insert_import(
    transaction: &mut Transaction<'_>,
    event: &AuditEvent,
) -> Result<(), ImportError> {
    insert_persisted(transaction, event).map_err(|_| ImportError::PersistenceFailure)
}

pub(crate) fn insert_enrollment(
    transaction: &mut Transaction<'_>,
    event: &AuditEvent,
) -> Result<(), EnrollmentError> {
    insert_persisted(transaction, event).map_err(EnrollmentError::from)
}

fn insert_persisted(
    transaction: &mut Transaction<'_>,
    event: &AuditEvent,
) -> Result<(), AuditStoreError> {
    let detail_json = serde_json::to_string(&event.detail).unwrap_or_else(|_| {
        tracing::error!(
            correlation_id = %event.correlation_id.as_text(),
            "audit detail serialization invariant failed"
        );
        panic!("audit detail serialization invariant failed");
    });

    diesel::insert_into(audit_events::table)
        .values((
            audit_events::audit_event_id.eq(event.audit_event_id_text()),
            audit_events::occurred_at.eq(sql::<Text>("strftime('%Y-%m-%dT%H:%M:%fZ', 'now')")),
            audit_events::actor.eq(event.actor),
            audit_events::action_kind.eq(event.action_kind),
            audit_events::resource_type.eq(event.resource_type),
            audit_events::resource_id.eq(event.resource_id.as_deref()),
            audit_events::result.eq(event.result),
            audit_events::reason_code.eq(event.reason_code),
            audit_events::correlation_id.eq(event.correlation_id.as_text()),
            audit_events::group_correlation_id.eq(event.group_correlation_id.as_deref()),
            audit_events::redacted_detail_json.eq(detail_json),
        ))
        .execute(transaction.connection())
        .map(|_| ())
        .map_err(|_| AuditStoreError::InsertFailed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditStoreError {
    InsertFailed,
}

impl From<AuditStoreError> for CommandError {
    fn from(source: AuditStoreError) -> Self {
        match source {
            AuditStoreError::InsertFailed => Self::PersistenceFailed,
        }
    }
}

impl From<AuditStoreError> for OperatorError {
    fn from(source: AuditStoreError) -> Self {
        match source {
            AuditStoreError::InsertFailed => Self::PersistenceFailed,
        }
    }
}

impl From<AuditStoreError> for DeviceError {
    fn from(source: AuditStoreError) -> Self {
        match source {
            AuditStoreError::InsertFailed => Self::PersistenceFailed,
        }
    }
}

impl From<AuditStoreError> for ProvisioningError {
    fn from(source: AuditStoreError) -> Self {
        match source {
            AuditStoreError::InsertFailed => Self::PersistenceFailed,
        }
    }
}

impl From<AuditStoreError> for EnrollmentError {
    fn from(source: AuditStoreError) -> Self {
        match source {
            AuditStoreError::InsertFailed => Self::PersistenceFailed,
        }
    }
}
