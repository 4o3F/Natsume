mod db;

use std::collections::HashMap;

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

    pub(crate) async fn materialize(
        &self,
        device_id: DeviceId,
    ) -> Result<SessionControlTarget, SessionControlError> {
        self.database
            .write(move |transaction| find_or_insert_target(transaction, &device_id))
            .await
            .map_err(TransactionError::into_error)
    }

    /// Reads the current durable target without creating the default target.
    pub(crate) async fn read_current(
        &self,
        device_id: DeviceId,
    ) -> Result<Option<SessionControlTarget>, SessionControlError> {
        self.database
            .read(move |transaction| {
                require_existing_device(transaction, &device_id)?;
                db::find_target(transaction, &device_id)?
                    .map(parse_target)
                    .transpose()
            })
            .await
            .map_err(TransactionError::into_error)
    }

    /// Reads and validates every initialized Session Control target in one query.
    pub(crate) async fn read_all_current(
        &self,
    ) -> Result<HashMap<DeviceId, SessionControlTarget>, SessionControlError> {
        self.database
            .read(|transaction| {
                let rows = db::list_targets(transaction)?;
                let mut targets = HashMap::with_capacity(rows.len());
                for (device_id, foreground_target, terminate_epoch) in rows {
                    let device_id = DeviceId::parse(&device_id)
                        .ok_or(SessionControlError::InvalidPersistedFacts)?;
                    let target = parse_target((foreground_target, terminate_epoch))?;
                    targets.insert(device_id, target);
                }
                Ok(targets)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    /// Applies an Operator submission inside the caller's write transaction.
    /// Every domain rejection precedes writes; write/invariant failures are fatal.
    pub(in crate::component) fn set_foreground_in_transaction(
        transaction: &mut Transaction<'_>,
        device_id: &DeviceId,
        foreground_target: ForegroundTarget,
    ) -> Result<SessionControlTarget, SessionControlError> {
        let mut target = find_or_insert_target(transaction, device_id)?;
        if target.foreground_target == foreground_target {
            return Ok(target);
        }
        let persisted = match foreground_target {
            ForegroundTarget::Contest => "contest",
            ForegroundTarget::Waiting => "waiting",
        };
        require_one(db::update_foreground_target(
            transaction,
            device_id,
            persisted,
        )?)?;
        target.foreground_target = foreground_target;
        Ok(target)
    }

    pub(in crate::component) fn terminate_in_transaction(
        transaction: &mut Transaction<'_>,
        device_id: &DeviceId,
    ) -> Result<SessionControlTarget, SessionControlError> {
        let mut target = find_or_insert_target(transaction, device_id)?;
        let next_epoch = match target.terminate_epoch {
            None => 1,
            Some(epoch) if epoch < i64::MAX.cast_unsigned() => epoch + 1,
            Some(_) => return Err(SessionControlError::TerminateEpochOverflow),
        };
        require_one(db::update_terminate_epoch(
            transaction,
            device_id,
            next_epoch.cast_signed(),
        )?)?;
        target.terminate_epoch = Some(next_epoch);
        Ok(target)
    }
}

fn find_or_insert_target(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<SessionControlTarget, SessionControlError> {
    require_existing_device(transaction, device_id)?;
    let Some((foreground_target, terminate_epoch)) = db::find_target(transaction, device_id)?
    else {
        require_one(db::insert_default_target(transaction, device_id)?)?;
        return Ok(SessionControlTarget {
            foreground_target: ForegroundTarget::Waiting,
            terminate_epoch: None,
        });
    };
    parse_target((foreground_target, terminate_epoch))
}

fn require_existing_device(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<(), SessionControlError> {
    if db::device_exists(transaction, device_id)? {
        Ok(())
    } else {
        Err(SessionControlError::DeviceNotFound)
    }
}

fn parse_target(
    (foreground_target, terminate_epoch): (String, Option<i64>),
) -> Result<SessionControlTarget, SessionControlError> {
    let foreground_target = match foreground_target.as_str() {
        "contest" => ForegroundTarget::Contest,
        "waiting" => ForegroundTarget::Waiting,
        _ => return Err(SessionControlError::InvalidPersistedFacts),
    };
    let terminate_epoch = match terminate_epoch {
        None => None,
        Some(epoch) if epoch > 0 => Some(epoch.cast_unsigned()),
        Some(_) => return Err(SessionControlError::InvalidPersistedFacts),
    };
    Ok(SessionControlTarget {
        foreground_target,
        terminate_epoch,
    })
}

fn require_one(updated: usize) -> Result<(), SessionControlError> {
    if updated == 1 {
        Ok(())
    } else {
        Err(SessionControlError::PersistenceFailed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForegroundTarget {
    Contest,
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionControlTarget {
    foreground_target: ForegroundTarget,
    terminate_epoch: Option<u64>,
}

impl SessionControlTarget {
    pub(crate) const fn foreground_target(&self) -> ForegroundTarget {
        self.foreground_target
    }

    pub(crate) const fn terminate_epoch(&self) -> Option<u64> {
        self.terminate_epoch
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum SessionControlError {
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
