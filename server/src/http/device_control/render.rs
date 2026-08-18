use natsume_device_protocol::generated::{
    Command, ControlEnvelope, LockSession, OpenBindingPrompt, ResetHome, SessionTarget, SyncState,
    TargetAssignment, TargetGateway, TargetSession, TargetStateSnapshot, TerminateSession,
    UnlockSession, command, control_envelope,
};
use serde::de::DeserializeOwned;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::application::command::{
    CommandKind, DispatchableCommand,
    validate::{
        LockSessionPayload, OpenBindingPromptPayload, PAYLOAD_VERSION, ResetHomePayload,
        SyncStatePayload, TerminateSessionPayload, UnlockSessionPayload, ValidatePayload,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RenderError {
    PayloadVersionUnsupported,
    PayloadCorrupt,
    TimestampCorrupt,
    HeldByPhasePolicy,
}

pub(super) fn render_wire_command(
    row: &DispatchableCommand,
) -> Result<ControlEnvelope, RenderError> {
    if row.payload_version != PAYLOAD_VERSION {
        return Err(RenderError::PayloadVersionUnsupported);
    }
    let created_at_unix_ms = parse_timestamp_unix_ms(&row.created_at)?;
    let deadline_unix_ms = row
        .deadline_at
        .as_deref()
        .map(parse_timestamp_unix_ms)
        .transpose()?
        .unwrap_or(0);
    let body = match row.kind {
        CommandKind::SyncState => command::Body::SyncState(render_sync_state(deserialize(row)?)?),
        CommandKind::SyncSecret => return Err(RenderError::HeldByPhasePolicy),
        CommandKind::OpenBindingPrompt => {
            let payload: OpenBindingPromptPayload = deserialize(row)?;
            command::Body::OpenBindingPrompt(OpenBindingPrompt {
                expires_at_unix_ms: payload.expires_at_unix_ms,
                prompt_message_id: payload.prompt_message_id,
            })
        }
        CommandKind::LockSession => {
            let payload: LockSessionPayload = deserialize(row)?;
            command::Body::LockSession(LockSession {
                target: Some(SessionTarget {
                    session_instance_id: payload.target.session_instance_id,
                    session_epoch: payload.target.session_epoch,
                }),
                requested_lock_epoch: payload.requested_lock_epoch,
            })
        }
        CommandKind::UnlockSession => {
            let payload: UnlockSessionPayload = deserialize(row)?;
            command::Body::UnlockSession(UnlockSession {
                target: Some(SessionTarget {
                    session_instance_id: payload.target.session_instance_id,
                    session_epoch: payload.target.session_epoch,
                }),
                expected_lock_epoch: payload.expected_lock_epoch,
                expected_lock_command_id: payload.expected_lock_command_id,
            })
        }
        CommandKind::TerminateSession => {
            let payload: TerminateSessionPayload = deserialize(row)?;
            command::Body::TerminateSession(TerminateSession {
                target: Some(SessionTarget {
                    session_instance_id: payload.target.session_instance_id,
                    session_epoch: payload.target.session_epoch,
                }),
            })
        }
        CommandKind::ResetHome => {
            let payload: ResetHomePayload = deserialize(row)?;
            command::Body::ResetHome(ResetHome {
                home_template_revision: payload.home_template_revision,
                home_epoch: payload.home_epoch,
            })
        }
    };
    Ok(ControlEnvelope {
        body: Some(control_envelope::Body::Command(Command {
            command_id: row.command_id.clone(),
            created_at_unix_ms,
            deadline_unix_ms,
            body: Some(body),
        })),
    })
}

fn render_sync_state(payload: SyncStatePayload) -> Result<SyncState, RenderError> {
    let canonical_hash =
        hex::decode(payload.canonical_hash).map_err(|_| RenderError::PayloadCorrupt)?;
    let snapshot = payload.snapshot;
    Ok(SyncState {
        generation: payload.generation,
        canonical_hash,
        snapshot: Some(TargetStateSnapshot {
            schema_version: snapshot.schema_version,
            assignment: Some(TargetAssignment {
                binding_revision: snapshot.assignment.binding_revision,
                seat_id: snapshot.assignment.seat_id,
                seat_code: snapshot.assignment.seat_code,
                account_id: snapshot.assignment.account_id,
                domjudge_username: snapshot.assignment.domjudge_username,
            }),
            gateway: Some(TargetGateway {
                gateway_configuration_revision: snapshot.gateway.gateway_configuration_revision,
                local_origin_hostname: snapshot.gateway.local_origin_hostname,
                fixed_upstream_profile_id: snapshot.gateway.fixed_upstream_profile_id,
                exact_login_policy_id: snapshot.gateway.exact_login_policy_id,
                gateway_certificate_profile_id: snapshot.gateway.gateway_certificate_profile_id,
                gateway_certificate_min_valid_until_unix_ms: snapshot
                    .gateway
                    .gateway_certificate_min_valid_until_unix_ms,
            }),
            session: Some(TargetSession {
                browser_policy_revision: snapshot.session.browser_policy_revision,
                home_template_revision: snapshot.session.home_template_revision,
            }),
        }),
    })
}

fn deserialize<T>(row: &DispatchableCommand) -> Result<T, RenderError>
where
    T: DeserializeOwned + ValidatePayload,
{
    let payload: T =
        serde_json::from_str(&row.frozen_payload_json).map_err(|_| RenderError::PayloadCorrupt)?;
    if !payload.validate() {
        return Err(RenderError::PayloadCorrupt);
    }
    Ok(payload)
}

fn parse_timestamp_unix_ms(value: &str) -> Result<i64, RenderError> {
    let timestamp =
        OffsetDateTime::parse(value, &Rfc3339).map_err(|_| RenderError::TimestampCorrupt)?;
    i64::try_from(timestamp.unix_timestamp_nanos().div_euclid(1_000_000))
        .map_err(|_| RenderError::TimestampCorrupt)
}

#[cfg(test)]
mod tests;
