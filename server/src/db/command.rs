use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, dsl::sql, sql_types::Text,
};
use snafu::Snafu;

use crate::{
    application::command::CommandError,
    audit::{self, AuditEvent},
    db::{
        Database,
        schema::{commands, devices},
    },
};

pub(crate) struct NewCommand {
    pub(crate) command_id: String,
    pub(crate) device_pk: String,
    pub(crate) kind: &'static str,
    pub(crate) request_fingerprint_version: i32,
    pub(crate) request_fingerprint_sha256: Vec<u8>,
    pub(crate) group_correlation_id: Option<String>,
    pub(crate) payload_version: i32,
    pub(crate) frozen_payload_json: String,
}

pub(crate) struct PersistedCommandRequest {
    pub(crate) request_fingerprint_version: i32,
    pub(crate) request_fingerprint_sha256: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InsertCommandOutcome {
    Inserted,
    /// The command row already exists: a concurrent PUT won the race between the caller's
    /// read and this insert. The whole transaction (including the created audit row) has
    /// been rolled back; the caller re-classifies against the persisted row.
    CommandIdExists,
}

pub(crate) async fn find_device_pk(
    database: &Database,
    device_id: &str,
) -> Result<Option<String>, CommandError> {
    let device_id = device_id.to_owned();
    database
        .interact(move |connection| {
            devices::table
                .select(devices::device_pk)
                .filter(devices::device_pk.eq(device_id))
                .first::<String>(connection)
                .optional()
                .map_err(|_| CommandStoreError::Read)
        })
        .await
        .map_err(|_| CommandStoreError::Acquire)?
        .map_err(CommandError::from)
}

pub(crate) async fn find_command(
    database: &Database,
    command_id: &str,
) -> Result<Option<PersistedCommandRequest>, CommandError> {
    let command_id = command_id.to_owned();
    database
        .interact(move |connection| {
            commands::table
                .select((
                    commands::request_fingerprint_version,
                    commands::request_fingerprint_sha256,
                ))
                .filter(commands::command_id.eq(command_id))
                .first::<(i32, Vec<u8>)>(connection)
                .optional()
                .map(|row| {
                    row.map(
                        |(request_fingerprint_version, request_fingerprint_sha256)| {
                            PersistedCommandRequest {
                                request_fingerprint_version,
                                request_fingerprint_sha256,
                            }
                        },
                    )
                })
                .map_err(|_| CommandStoreError::Read)
        })
        .await
        .map_err(|_| CommandStoreError::Acquire)?
        .map_err(CommandError::from)
}

pub(crate) async fn insert_command_with_created_audit(
    database: &Database,
    command: NewCommand,
    event: AuditEvent,
) -> Result<InsertCommandOutcome, CommandError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                let created_audit_event_id = event.audit_event_id_text();
                audit::insert_diesel(connection, &event)
                    .map_err(|_| CommandStoreError::AuditInsert)?;
                diesel::insert_into(commands::table)
                    .values((
                        commands::command_id.eq(command.command_id),
                        commands::device_pk.eq(command.device_pk),
                        commands::kind.eq(command.kind),
                        commands::state.eq("created"),
                        commands::request_fingerprint_version
                            .eq(command.request_fingerprint_version),
                        commands::request_fingerprint_sha256.eq(command.request_fingerprint_sha256),
                        commands::group_correlation_id.eq(command.group_correlation_id.as_deref()),
                        commands::payload_version.eq(command.payload_version),
                        commands::frozen_payload_json.eq(command.frozen_payload_json),
                        commands::created_at
                            .eq(sql::<Text>("strftime('%Y-%m-%dT%H:%M:%fZ', 'now')")),
                        commands::deadline_at.eq(Option::<&str>::None),
                        commands::terminal_error_code.eq(Option::<&str>::None),
                        commands::redacted_terminal_result_json.eq(Option::<&str>::None),
                        commands::created_audit_event_id.eq(created_audit_event_id),
                    ))
                    .execute(connection)
                    .map_err(|error| match error {
                        // Any unique violation inside this insert means the command row
                        // already exists (the audit event ID is a freshly generated
                        // UUIDv7); returning an error rolls the audit row back with it.
                        diesel::result::Error::DatabaseError(
                            diesel::result::DatabaseErrorKind::UniqueViolation,
                            _,
                        ) => CommandStoreError::CommandIdExists,
                        _ => CommandStoreError::Insert,
                    })?;
                Ok::<(), CommandStoreError>(())
            })
        })
        .await
        .map_err(|_| CommandStoreError::Acquire)?
        .map(|()| InsertCommandOutcome::Inserted)
        .or_else(|error| match error {
            CommandStoreError::CommandIdExists => Ok(InsertCommandOutcome::CommandIdExists),
            other => Err(CommandError::from(other)),
        })
}

pub(crate) async fn insert_command_conflict_audit(
    database: &Database,
    event: AuditEvent,
) -> Result<(), CommandError> {
    database
        .interact(move |connection| {
            connection.immediate_transaction(|connection| {
                audit::insert_diesel(connection, &event).map_err(|_| CommandStoreError::AuditInsert)
            })
        })
        .await
        .map_err(|_| CommandStoreError::Acquire)?
        .map_err(CommandError::from)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum CommandStoreError {
    #[snafu(display("the command database connection could not be acquired"))]
    Acquire,
    #[snafu(display("the command transaction failed"))]
    Transaction,
    #[snafu(display("command facts could not be read"))]
    Read,
    #[snafu(display("the command audit could not be inserted"))]
    AuditInsert,
    #[snafu(display("the command could not be inserted"))]
    Insert,
    #[snafu(display("the command row already exists"))]
    CommandIdExists,
}

impl From<diesel::result::Error> for CommandStoreError {
    fn from(_source: diesel::result::Error) -> Self {
        Self::Transaction
    }
}

#[cfg(test)]
mod tests;
