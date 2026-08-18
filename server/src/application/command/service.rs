use uuid::Uuid;

use crate::{
    application::device::DeviceState,
    audit::{AuditEvent, AuditEventId, CorrelationId},
    db::{self, Database},
};

use super::{
    types::{
        CommandError, CommandId, CommandOutcome, CommandRequestInput, CommandStatusWrite,
        CommandStatusWriteOutcome, DeviceCommandDispatchNotifier, DispatchableCommand,
        REQUEST_FINGERPRINT_VERSION, TransitionDecision,
    },
    validate::validate_request,
};

enum PutTransactionOutcome {
    Created {
        device_pk: String,
    },
    Replayed,
    Conflict {
        group_correlation_id: Option<String>,
    },
}

pub(crate) async fn put_command<N>(
    database: &Database,
    command_id: &CommandId,
    input: CommandRequestInput,
    correlation_id: CorrelationId,
    dispatch_notifier: &N,
) -> Result<CommandOutcome, CommandError>
where
    N: DeviceCommandDispatchNotifier,
{
    let request = validate_request(input)?;
    let command_id = command_id.value();
    let outcome = database
        .write(move |transaction| {
            let Some(device_state) =
                db::device::devices::find_state(transaction, &request.device_id)
                    .map_err(CommandError::from_device_persistence)?
            else {
                return Err(CommandError::DeviceNotFound);
            };
            if let Some(existing) = db::command::find_request_fingerprint(transaction, command_id)?
            {
                if existing == request.fingerprint {
                    return Ok(PutTransactionOutcome::Replayed);
                }
                return Ok(PutTransactionOutcome::Conflict {
                    group_correlation_id: request.group_correlation_id,
                });
            }
            if device_state != DeviceState::Enrolled {
                return Err(CommandError::DeviceNotEnrolled);
            }
            let device_pk = request.device_id.as_text();

            let created_audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
            let event = AuditEvent::command_created(
                created_audit_event_id,
                correlation_id,
                command_id,
                request.group_correlation_id.clone(),
                request.kind.as_str(),
                request.payload_version,
                request.fingerprint.version,
            );
            db::audit::insert(transaction, &event).map_err(CommandError::from_audit_persistence)?;
            db::command::insert(
                transaction,
                command_id,
                &device_pk,
                &request,
                created_audit_event_id,
            )?;
            Ok(PutTransactionOutcome::Created { device_pk })
        })
        .await?;
    match outcome {
        PutTransactionOutcome::Created { device_pk } => {
            dispatch_notifier.notify_command_dispatch(&device_pk);
            Ok(CommandOutcome::Created)
        }
        PutTransactionOutcome::Replayed => Ok(CommandOutcome::Replayed),
        PutTransactionOutcome::Conflict {
            group_correlation_id,
        } => {
            let event = AuditEvent::command_request_conflict(
                AuditEventId::from_uuid(Uuid::now_v7()),
                correlation_id,
                command_id,
                group_correlation_id,
                REQUEST_FINGERPRINT_VERSION,
            );
            database
                .write(move |transaction| {
                    db::audit::insert(transaction, &event)
                        .map_err(CommandError::from_audit_persistence)
                })
                .await?;
            Err(CommandError::RequestConflict)
        }
    }
}

pub(crate) async fn list_dispatchable_commands(
    database: &Database,
    device_pk: &str,
) -> Result<Vec<DispatchableCommand>, CommandError> {
    let device_pk = device_pk.to_owned();
    database
        .read(move |transaction| db::command::list_dispatchable_commands(transaction, &device_pk))
        .await
}

pub(crate) async fn writeback_command_status(
    database: &Database,
    device_pk: &str,
    status: CommandStatusWrite,
) -> Result<CommandStatusWriteOutcome, CommandError> {
    let device_pk = device_pk.to_owned();
    let correlation_id = CorrelationId::from_uuid(Uuid::now_v7());
    database
        .write(move |transaction| {
            let Some(current) = db::command::find_lifecycle_facts(transaction, status.command_id)?
            else {
                return Ok(CommandStatusWriteOutcome::IgnoredUnknownCommand);
            };
            if current.device_pk.as_text() != device_pk {
                return Ok(CommandStatusWriteOutcome::IgnoredForeignCommand);
            }
            match current.state.classify(status.state) {
                TransitionDecision::DuplicateNoop => {
                    return Ok(CommandStatusWriteOutcome::IgnoredTransition);
                }
                TransitionDecision::Regression => {
                    return Ok(CommandStatusWriteOutcome::IgnoredRegression);
                }
                TransitionDecision::Apply => {}
            }

            db::command::update_status(transaction, &device_pk, &status)?;
            if !status.state.is_terminal() {
                return Ok(CommandStatusWriteOutcome::UpdatedNonterminal);
            }
            let event = AuditEvent::command_terminal(
                AuditEventId::from_uuid(Uuid::now_v7()),
                correlation_id,
                status.command_id,
                current.kind.as_str().to_owned(),
                status.state.as_str(),
                status.terminal_error_code.clone(),
            );
            db::audit::insert(transaction, &event).map_err(CommandError::from_audit_persistence)?;
            Ok(CommandStatusWriteOutcome::UpdatedTerminal)
        })
        .await
}
