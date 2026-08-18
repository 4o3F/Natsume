use diesel::{ExpressionMethods, RunQueryDsl, dsl::sql, sql_types::Text};

use crate::{
    audit::{AuditEvent, AuditPersistenceError},
    db::{Transaction, schema::audit_events},
};

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    event: &AuditEvent,
) -> Result<(), AuditPersistenceError> {
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
        .map_err(|_| AuditPersistenceError::PersistenceFailed)
}
