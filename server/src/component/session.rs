mod db;

use snafu::Snafu;

use crate::{
    component::device::DeviceId,
    db::{Database, PersistenceError, Transaction, TransactionError},
};

pub(crate) struct SessionControlComponent {
    database: Database,
}

impl SessionControlComponent {
    pub(crate) const fn new(database: Database) -> Self {
        Self { database }
    }

    async fn materialize(
        &self,
        device_id: DeviceId,
    ) -> Result<SessionControlTarget, SessionControlError> {
        self.database
            .write(move |transaction| find_or_insert_target(transaction, &device_id))
            .await
            .map_err(TransactionError::into_error)
    }

    async fn set_lock(
        &self,
        device_id: DeviceId,
        lock_state: LockState,
    ) -> Result<SessionControlTarget, SessionControlError> {
        self.database
            .write(move |transaction| {
                let mut target = find_or_insert_target(transaction, &device_id)?;
                if target.lock_state == lock_state {
                    return Ok(target);
                }
                let persisted_lock_state = match lock_state {
                    LockState::Unlocked => "unlocked",
                    LockState::Locked => "locked",
                };
                require_one(db::update_lock_state(
                    transaction,
                    &device_id,
                    persisted_lock_state,
                )?)?;
                target.lock_state = lock_state;
                Ok(target)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    async fn terminate(
        &self,
        device_id: DeviceId,
    ) -> Result<SessionControlTarget, SessionControlError> {
        self.database
            .write(move |transaction| {
                let mut target = find_or_insert_target(transaction, &device_id)?;
                let next_epoch = match target.terminate_epoch {
                    None => 1,
                    Some(epoch) if epoch < i64::MAX.cast_unsigned() => epoch + 1,
                    Some(_) => return Err(SessionControlError::TerminateEpochOverflow),
                };
                require_one(db::update_terminate_epoch(
                    transaction,
                    &device_id,
                    next_epoch.cast_signed(),
                )?)?;
                target.terminate_epoch = Some(next_epoch);
                Ok(target)
            })
            .await
            .map_err(TransactionError::into_error)
    }
}

fn find_or_insert_target(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<SessionControlTarget, SessionControlError> {
    if !db::device_exists(transaction, device_id)? {
        return Err(SessionControlError::DeviceNotFound);
    }
    let Some((lock_state, terminate_epoch)) = db::find_target(transaction, device_id)? else {
        require_one(db::insert_default_target(transaction, device_id)?)?;
        return Ok(SessionControlTarget {
            lock_state: LockState::Unlocked,
            terminate_epoch: None,
        });
    };
    let lock_state = match lock_state.as_str() {
        "unlocked" => LockState::Unlocked,
        "locked" => LockState::Locked,
        _ => return Err(SessionControlError::InvalidPersistedFacts),
    };
    let terminate_epoch = match terminate_epoch {
        None => None,
        Some(epoch) if epoch > 0 => Some(epoch.cast_unsigned()),
        Some(_) => return Err(SessionControlError::InvalidPersistedFacts),
    };
    Ok(SessionControlTarget {
        lock_state,
        terminate_epoch,
    })
}

fn require_one(updated: usize) -> Result<(), SessionControlError> {
    if updated == 1 {
        Ok(())
    } else {
        Err(SessionControlError::InvalidPersistedFacts)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockState {
    Unlocked,
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionControlTarget {
    lock_state: LockState,
    terminate_epoch: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
enum SessionControlError {
    #[snafu(display("the Device does not exist"))]
    DeviceNotFound,
    #[snafu(display("the terminate epoch cannot advance further"))]
    TerminateEpochOverflow,
    #[snafu(display("persisted Session Control facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("Session Control persistence failed"))]
    PersistenceFailed,
}

impl From<PersistenceError> for SessionControlError {
    fn from(error: PersistenceError) -> Self {
        match error {
            PersistenceError::InvalidPersistedData => Self::InvalidPersistedFacts,
            PersistenceError::OperationFailed => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
mod tests;
