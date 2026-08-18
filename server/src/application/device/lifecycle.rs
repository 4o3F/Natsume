use uuid::Uuid;

use crate::{
    audit::{AuditDetail, AuditEvent, AuditEventId, CorrelationId, DeviceLifecycleAuditResult},
    db::{self, Database},
};

use super::{DeviceConnectionEvictor, DeviceError, DeviceId, DeviceState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceLifecycleAction {
    Revoke,
    Disable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceLifecycleFacts {
    pub(crate) state: DeviceState,
    pub(crate) token_count: i64,
    pub(crate) non_revoked_certificate_count: i64,
}

/// The whole lifecycle transition: whether it writes at all, the state the
/// Device ends in, and the exact row counts the mutation must observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceLifecycleOutcome {
    pub(crate) applies: bool,
    pub(crate) resulting_state: DeviceState,
    pub(crate) removed_token_count: i64,
    pub(crate) revoked_certificate_count: i64,
}

/// Decides a Device lifecycle transition from current Server facts only.
#[must_use]
pub(crate) const fn decide_device_lifecycle(
    action: DeviceLifecycleAction,
    current: DeviceLifecycleFacts,
) -> DeviceLifecycleOutcome {
    match action {
        DeviceLifecycleAction::Revoke => {
            if matches!(current.state, DeviceState::Revoked)
                && current.token_count == 0
                && current.non_revoked_certificate_count == 0
            {
                DeviceLifecycleOutcome {
                    applies: false,
                    resulting_state: DeviceState::Revoked,
                    removed_token_count: 0,
                    revoked_certificate_count: 0,
                }
            } else {
                DeviceLifecycleOutcome {
                    applies: true,
                    resulting_state: DeviceState::Revoked,
                    removed_token_count: current.token_count,
                    revoked_certificate_count: current.non_revoked_certificate_count,
                }
            }
        }
        DeviceLifecycleAction::Disable => match current.state {
            DeviceState::Enrolled => DeviceLifecycleOutcome {
                applies: true,
                resulting_state: DeviceState::Disabled,
                removed_token_count: 0,
                revoked_certificate_count: 0,
            },
            DeviceState::Disabled | DeviceState::Revoked => DeviceLifecycleOutcome {
                applies: false,
                resulting_state: current.state,
                removed_token_count: 0,
                revoked_certificate_count: 0,
            },
        },
    }
}

pub(crate) async fn revoke_device<E>(
    database: &Database,
    device_id: &DeviceId,
    correlation_id: CorrelationId,
    connection_evictor: &E,
) -> Result<(), DeviceError>
where
    E: DeviceConnectionEvictor + ?Sized,
{
    apply_device_lifecycle(
        database,
        device_id,
        DeviceLifecycleAction::Revoke,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await?;
    let _evicted = connection_evictor.evict_device_connection(&device_id.as_text());
    Ok(())
}

pub(crate) async fn disable_device<E>(
    database: &Database,
    device_id: &DeviceId,
    correlation_id: CorrelationId,
    connection_evictor: &E,
) -> Result<(), DeviceError>
where
    E: DeviceConnectionEvictor + ?Sized,
{
    apply_device_lifecycle(
        database,
        device_id,
        DeviceLifecycleAction::Disable,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await?;
    let _evicted = connection_evictor.evict_device_connection(&device_id.as_text());
    Ok(())
}

async fn apply_device_lifecycle(
    database: &Database,
    device_id: &DeviceId,
    action: DeviceLifecycleAction,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<(), DeviceError> {
    let device_id = device_id.as_text();
    database
        .write(move |transaction| {
            let facts = db::device::query::find_device_lifecycle(transaction, &device_id)
                .map_err(DeviceError::from_persistence)?
                .ok_or(DeviceError::DeviceNotFound)?;
            let outcome = decide_device_lifecycle(action, facts);
            let audit_result = if outcome.applies {
                DeviceLifecycleAuditResult::Succeeded
            } else {
                DeviceLifecycleAuditResult::Noop
            };
            let event = AuditEvent::device_lifecycle(
                audit_event_id,
                correlation_id,
                device_id.clone(),
                action,
                audit_result,
                AuditDetail::DeviceLifecycle {
                    resulting_state: outcome.resulting_state.as_persisted(),
                    removed_token_count: outcome.removed_token_count,
                    revoked_certificate_count: outcome.revoked_certificate_count,
                },
            );
            db::audit::insert(transaction, &event).map_err(DeviceError::from_audit_persistence)?;

            if !outcome.applies {
                return Ok(());
            }
            db::device::devices::update_state_guarded(
                transaction,
                &device_id,
                facts.state,
                outcome.resulting_state,
            )
            .map_err(DeviceError::from_persistence)?;
            if action == DeviceLifecycleAction::Disable {
                return Ok(());
            }

            let removed = db::device::tokens::delete(transaction, &device_id)
                .map_err(DeviceError::from_persistence)?;
            if removed != outcome.removed_token_count {
                return Err(DeviceError::PersistenceFailed);
            }
            let revoked = db::device::certificates::revoke_non_revoked(transaction, &device_id)
                .map_err(DeviceError::from_persistence)?;
            if revoked != outcome.revoked_certificate_count {
                return Err(DeviceError::PersistenceFailed);
            }
            Ok(())
        })
        .await
}

#[cfg(test)]
#[path = "lifecycle/tests.rs"]
mod tests;
