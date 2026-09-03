mod db;

use std::collections::HashMap;

use snafu::Snafu;

use crate::{
    component::device::DeviceId,
    db::{Database, PersistenceError, Transaction, TransactionError},
};

pub(crate) struct HomeComponent {
    database: Database,
}

impl HomeComponent {
    pub(crate) const fn new(database: Database) -> Self {
        Self { database }
    }

    pub(crate) async fn materialize(&self, device_id: DeviceId) -> Result<Option<u64>, HomeError> {
        self.database
            .write(move |transaction| {
                require_existing_device(transaction, &device_id)?;
                if let Some(row) = db::find_reset_epoch(transaction, &device_id)? {
                    parse_reset_epoch(row)
                } else {
                    require_one(db::insert_target(transaction, &device_id, None)?)?;
                    Ok(None)
                }
            })
            .await
            .map_err(TransactionError::into_error)
    }

    /// Reads the current durable reset epoch without creating a target row.
    pub(crate) async fn read_current(&self, device_id: DeviceId) -> Result<Option<u64>, HomeError> {
        self.database
            .read(move |transaction| {
                require_existing_device(transaction, &device_id)?;
                db::find_reset_epoch(transaction, &device_id)?
                    .map(parse_reset_epoch)
                    .transpose()
                    .map(Option::flatten)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    /// Reads and validates every initialized Home target in one query.
    pub(crate) async fn read_all_current(
        &self,
    ) -> Result<HashMap<DeviceId, Option<u64>>, HomeError> {
        self.database
            .read(|transaction| {
                let rows = db::list_reset_epochs(transaction)?;
                let mut targets = HashMap::with_capacity(rows.len());
                for row in rows {
                    let device_id =
                        DeviceId::parse(&row.0).ok_or(HomeError::InvalidPersistedFacts)?;
                    let target = parse_reset_epoch(row)?;
                    targets.insert(device_id, target);
                }
                Ok(targets)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    /// Advances the durable Home reset epoch for the Device.
    pub(crate) async fn reset(&self, device_id: DeviceId) -> Result<u64, HomeError> {
        self.database
            .write(move |transaction| {
                require_existing_device(transaction, &device_id)?;
                let Some((_, persisted_epoch)) = db::find_reset_epoch(transaction, &device_id)?
                else {
                    require_one(db::insert_target(transaction, &device_id, Some(1))?)?;
                    return Ok(1);
                };
                let next_epoch = match persisted_epoch {
                    None => 1,
                    Some(epoch) if epoch > 0 => {
                        epoch.checked_add(1).ok_or(HomeError::EpochExhausted)?
                    }
                    Some(_) => return Err(HomeError::InvalidPersistedFacts),
                };
                require_one(db::set_reset_epoch(transaction, &device_id, next_epoch)?)?;
                Ok(next_epoch.cast_unsigned())
            })
            .await
            .map_err(TransactionError::into_error)
    }
}

fn parse_reset_epoch((_, epoch): (String, Option<i64>)) -> Result<Option<u64>, HomeError> {
    match epoch {
        None => Ok(None),
        Some(epoch) if epoch > 0 => Ok(Some(epoch.cast_unsigned())),
        Some(_) => Err(HomeError::InvalidPersistedFacts),
    }
}

fn require_existing_device(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<(), HomeError> {
    if db::device_exists(transaction, device_id)? {
        Ok(())
    } else {
        Err(HomeError::DeviceNotFound)
    }
}

fn require_one(updated: usize) -> Result<(), HomeError> {
    if updated == 1 {
        Ok(())
    } else {
        Err(HomeError::InvalidPersistedFacts)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum HomeError {
    #[snafu(display("the Device does not exist"))]
    DeviceNotFound,
    #[snafu(display("the Home reset epoch is exhausted"))]
    EpochExhausted,
    #[snafu(display("persisted Home facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("Home persistence failed"))]
    PersistenceFailed,
}

impl From<PersistenceError> for HomeError {
    fn from(error: PersistenceError) -> Self {
        match error {
            PersistenceError::InvalidPersistedData => Self::InvalidPersistedFacts,
            PersistenceError::OperationFailed => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
mod tests;
