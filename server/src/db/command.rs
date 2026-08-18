use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, dsl::sql, sql_types::Text,
};
use uuid::Uuid;

use crate::{
    application::command::{
        CommandError, CommandKind, CommandLifecycleFacts, CommandRequestFingerprint,
        CommandStatusWrite, DispatchableCommand, ValidatedCommandRequest,
    },
    application::device::DeviceId,
    audit::AuditEventId,
    db::{DatabaseError, Transaction, schema::commands},
};

pub(crate) const DISPATCH_BATCH_LIMIT: usize = 256;

pub(crate) fn find_request_fingerprint(
    transaction: &mut Transaction<'_>,
    command_id: Uuid,
) -> Result<Option<CommandRequestFingerprint>, CommandError> {
    commands::table
        .select((
            commands::request_fingerprint_version,
            commands::request_fingerprint_sha256,
        ))
        .filter(commands::command_id.eq(command_id.to_string()))
        .first::<(i32, Vec<u8>)>(transaction.connection())
        .optional()
        .map(|row| {
            row.map(
                |(request_fingerprint_version, request_fingerprint_sha256)| {
                    CommandRequestFingerprint {
                        version: request_fingerprint_version,
                        sha256: request_fingerprint_sha256,
                    }
                },
            )
        })
        .map_err(|_| CommandError::PersistenceFailed)
}

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    command_id: Uuid,
    device_pk: &str,
    request: &ValidatedCommandRequest,
    created_audit_event_id: AuditEventId,
) -> Result<(), CommandError> {
    diesel::insert_into(commands::table)
        .values((
            commands::command_id.eq(command_id.to_string()),
            commands::device_pk.eq(device_pk),
            commands::kind.eq(request.kind.as_str()),
            commands::state.eq("created"),
            commands::request_fingerprint_version.eq(request.fingerprint.version),
            commands::request_fingerprint_sha256.eq(&request.fingerprint.sha256),
            commands::group_correlation_id.eq(request.group_correlation_id.as_deref()),
            commands::payload_version.eq(request.payload_version),
            commands::frozen_payload_json.eq(&request.frozen_payload_json),
            commands::created_at.eq(sql::<Text>("strftime('%Y-%m-%dT%H:%M:%fZ', 'now')")),
            commands::deadline_at.eq(Option::<&str>::None),
            commands::terminal_error_code.eq(Option::<&str>::None),
            commands::redacted_terminal_result_json.eq(Option::<&str>::None),
            commands::created_audit_event_id.eq(created_audit_event_id.as_text()),
        ))
        .execute(transaction.connection())
        .map(|_| ())
        .map_err(|_| CommandError::PersistenceFailed)
}

pub(crate) fn list_dispatchable_commands(
    transaction: &mut Transaction<'_>,
    device_pk: &str,
) -> Result<Vec<DispatchableCommand>, CommandError> {
    let query_limit = i64::try_from(DISPATCH_BATCH_LIMIT.saturating_add(1))
        .map_err(|_| CommandError::PersistenceFailed)?;
    let mut rows = commands::table
        .select((
            commands::command_id,
            commands::kind,
            commands::payload_version,
            commands::frozen_payload_json,
            commands::created_at,
            commands::deadline_at,
        ))
        .filter(commands::device_pk.eq(device_pk))
        .filter(commands::state.eq_any(["created", "received", "running"]))
        .order_by((commands::created_at.asc(), commands::command_id.asc()))
        .limit(query_limit)
        .load::<(String, String, i32, String, String, Option<String>)>(transaction.connection())
        .map_err(|_| CommandError::PersistenceFailed)?;
    if rows.len() > DISPATCH_BATCH_LIMIT {
        tracing::debug!(
            batch_limit = DISPATCH_BATCH_LIMIT,
            "Device command dispatch batch was truncated; identifiers redacted"
        );
        rows.truncate(DISPATCH_BATCH_LIMIT);
    }
    rows.into_iter()
        .map(
            |(command_id, kind, payload_version, frozen_payload_json, created_at, deadline_at)| {
                Ok(DispatchableCommand {
                    command_id,
                    kind: CommandKind::parse_persisted(&kind)?,
                    payload_version,
                    frozen_payload_json,
                    created_at,
                    deadline_at,
                })
            },
        )
        .collect()
}

pub(crate) fn find_lifecycle_facts(
    transaction: &mut Transaction<'_>,
    command_id: Uuid,
) -> Result<Option<CommandLifecycleFacts>, CommandError> {
    let row = commands::table
        .select((commands::device_pk, commands::kind, commands::state))
        .filter(commands::command_id.eq(command_id.to_string()))
        .first::<(String, String, String)>(transaction.connection())
        .optional()
        .map_err(|_| CommandError::PersistenceFailed)?;
    row.map(|(device_pk, kind, state)| {
        Ok(CommandLifecycleFacts {
            device_pk: DeviceId::parse(&device_pk).ok_or(CommandError::PersistenceFailed)?,
            kind: CommandKind::parse_persisted(&kind)?,
            state: crate::application::command::CommandLifecycleState::parse_persisted(&state)?,
        })
    })
    .transpose()
}

pub(crate) fn update_status(
    transaction: &mut Transaction<'_>,
    device_pk: &str,
    status: &CommandStatusWrite,
) -> Result<(), CommandError> {
    let command_id = status.command_id.to_string();
    let target = commands::table
        .filter(commands::command_id.eq(command_id))
        .filter(commands::device_pk.eq(device_pk));
    let affected = if status.state.is_terminal() {
        diesel::update(target)
            .set((
                commands::state.eq(status.state.as_str()),
                commands::terminal_error_code.eq(status.terminal_error_code.as_deref()),
                commands::redacted_terminal_result_json.eq(Option::<&str>::None),
            ))
            .execute(transaction.connection())
    } else {
        diesel::update(target)
            .set(commands::state.eq(status.state.as_str()))
            .execute(transaction.connection())
    }
    .map_err(|_| CommandError::PersistenceFailed)?;
    if affected != 1 {
        return Err(CommandError::PersistenceFailed);
    }
    Ok(())
}

impl From<DatabaseError> for CommandError {
    fn from(error: DatabaseError) -> Self {
        match error {
            DatabaseError::InvalidConfiguration
            | DatabaseError::ConnectionFailed
            | DatabaseError::MigrationFailed
            | DatabaseError::TransactionFailed => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
mod tests;
