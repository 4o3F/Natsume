mod db;

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

    async fn materialize(&self, device_id: DeviceId) -> Result<Option<u64>, HomeError> {
        self.database
            .write(move |transaction| {
                require_existing_device(transaction, &device_id)?;
                match db::find_reset_epoch(transaction, &device_id)? {
                    Some((_, None)) => Ok(None),
                    Some((_, Some(epoch))) if epoch > 0 => Ok(Some(epoch.cast_unsigned())),
                    Some(_) => Err(HomeError::InvalidPersistedFacts),
                    None => {
                        require_one(db::insert_target(transaction, &device_id, None)?)?;
                        Ok(None)
                    }
                }
            })
            .await
            .map_err(TransactionError::into_error)
    }

    async fn reset(&self, device_id: DeviceId) -> Result<u64, HomeError> {
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
enum HomeError {
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
