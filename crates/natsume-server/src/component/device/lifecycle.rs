use crate::db::{Database, PersistenceError, Transaction, TransactionError};

use super::{DeviceError, DeviceId, DeviceState, LifecycleOutcome, db};

pub(super) async fn enable(
    database: &Database,
    device_id: DeviceId,
) -> Result<LifecycleOutcome, DeviceError> {
    set_non_terminal_state(database, device_id, DeviceState::Enabled).await
}

pub(super) async fn disable(
    database: &Database,
    device_id: DeviceId,
) -> Result<LifecycleOutcome, DeviceError> {
    set_non_terminal_state(database, device_id, DeviceState::Disabled).await
}

pub(super) async fn revoke(
    database: &Database,
    device_id: DeviceId,
) -> Result<LifecycleOutcome, DeviceError> {
    database
        .write(move |transaction| revoke_in_transaction(transaction, &device_id))
        .await
        .map_err(TransactionError::into_error)
}

async fn set_non_terminal_state(
    database: &Database,
    device_id: DeviceId,
    next: DeviceState,
) -> Result<LifecycleOutcome, DeviceError> {
    database
        .write(move |transaction| {
            let device =
                db::find_by_id(transaction, &device_id)?.ok_or(DeviceError::DeviceNotFound)?;
            match device.state() {
                DeviceState::Revoked => Ok(LifecycleOutcome::RejectedTerminal),
                current if current == next => {
                    require_current_key(transaction, &device_id)?;
                    Ok(LifecycleOutcome::Unchanged)
                }
                _ => {
                    require_current_key(transaction, &device_id)?;
                    require_one(db::update_state(transaction, &device_id, next)?)?;
                    Ok(LifecycleOutcome::Changed)
                }
            }
        })
        .await
        .map_err(TransactionError::into_error)
}

fn revoke_in_transaction(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<LifecycleOutcome, DeviceError> {
    let device = db::find_by_id(transaction, device_id)?.ok_or(DeviceError::DeviceNotFound)?;
    if device.state() == DeviceState::Revoked {
        if db::find_current_for_device(transaction, device_id)?.is_some() {
            return Err(DeviceError::InvalidPersistedFacts);
        }
        return Ok(LifecycleOutcome::Unchanged);
    }

    let current_key = require_current_key(transaction, device_id)?;
    let now = db::current_unix_ms(transaction)?;
    require_one(db::update_state(
        transaction,
        device_id,
        DeviceState::Revoked,
    )?)?;
    require_one(db::retire_current(
        transaction,
        &current_key,
        device_id,
        now,
    )?)?;
    Ok(LifecycleOutcome::Changed)
}

fn require_current_key(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<super::ControlPublicKey, DeviceError> {
    db::find_current_for_device(transaction, device_id)?.ok_or(DeviceError::InvalidPersistedFacts)
}

fn require_one(updated: usize) -> Result<(), DeviceError> {
    if updated == 1 {
        Ok(())
    } else {
        Err(PersistenceError::InvalidPersistedData.into())
    }
}
