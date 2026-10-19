mod db;

use snafu::Snafu;

use crate::{
    component::device::DeviceId,
    db::{Database, PersistenceError, Transaction, TransactionError},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PowerControlTarget {
    shutdown_epoch: Option<u64>,
    expires_at_unix_ms: Option<i64>,
}

impl PowerControlTarget {
    pub(crate) const fn shutdown_epoch(self) -> Option<u64> {
        self.shutdown_epoch
    }
    pub(crate) const fn expires_at_unix_ms(self) -> Option<i64> {
        self.expires_at_unix_ms
    }
}

pub(crate) struct PowerControlComponent {
    database: Database,
}

impl PowerControlComponent {
    pub(crate) const fn new(database: Database) -> Self {
        Self { database }
    }

    pub(crate) async fn materialize(
        &self,
        device_id: DeviceId,
    ) -> Result<PowerControlTarget, PowerError> {
        self.database
            .write(move |transaction| find_or_insert(transaction, &device_id))
            .await
            .map_err(TransactionError::into_error)
    }

    pub(in crate::component) fn request_shutdown_in_transaction(
        transaction: &mut Transaction<'_>,
        device_id: &DeviceId,
        expires_at_unix_ms: i64,
    ) -> Result<PowerControlTarget, PowerError> {
        if expires_at_unix_ms <= 0 {
            return Err(PowerError::InvalidDeadline);
        }
        let target = find_or_insert(transaction, device_id)?;
        let next = target.shutdown_epoch.map_or_else(
            || Ok(1),
            |epoch| epoch.checked_add(1).ok_or(PowerError::EpochExhausted),
        )?;
        require_one(db::update(
            transaction,
            device_id,
            next.cast_signed(),
            expires_at_unix_ms,
        )?)?;
        Ok(PowerControlTarget {
            shutdown_epoch: Some(next),
            expires_at_unix_ms: Some(expires_at_unix_ms),
        })
    }
}

fn find_or_insert(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<PowerControlTarget, PowerError> {
    if !db::device_exists(transaction, device_id)? {
        return Err(PowerError::DeviceNotFound);
    }
    if let Some(value) = db::find(transaction, device_id)? {
        parse(value)
    } else {
        require_one(db::insert(transaction, device_id)?)?;
        Ok(PowerControlTarget {
            shutdown_epoch: None,
            expires_at_unix_ms: None,
        })
    }
}

fn parse((epoch, deadline): (Option<i64>, Option<i64>)) -> Result<PowerControlTarget, PowerError> {
    match (epoch, deadline) {
        (None, None) => Ok(PowerControlTarget {
            shutdown_epoch: None,
            expires_at_unix_ms: None,
        }),
        (Some(epoch), Some(deadline)) if epoch > 0 && deadline > 0 => Ok(PowerControlTarget {
            shutdown_epoch: Some(epoch.cast_unsigned()),
            expires_at_unix_ms: Some(deadline),
        }),
        _ => Err(PowerError::InvalidPersistedFacts),
    }
}
fn require_one(rows: usize) -> Result<(), PowerError> {
    if rows == 1 {
        Ok(())
    } else {
        Err(PowerError::PersistenceFailed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum PowerError {
    #[snafu(display("the Device does not exist"))]
    DeviceNotFound,
    #[snafu(display("the shutdown epoch is exhausted"))]
    EpochExhausted,
    #[snafu(display("the shutdown deadline is invalid"))]
    InvalidDeadline,
    #[snafu(display("persisted Power Control facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("Power Control persistence failed"))]
    PersistenceFailed,
}
impl From<PersistenceError> for PowerError {
    fn from(value: PersistenceError) -> Self {
        match value {
            PersistenceError::InvalidPersistedData => Self::InvalidPersistedFacts,
            PersistenceError::OperationFailed => Self::PersistenceFailed,
        }
    }
}
