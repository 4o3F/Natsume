use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl};

use crate::{
    component::device::DeviceId,
    db::{PersistenceError, Transaction},
    diesel_schema::{device_home_targets, devices},
};

pub(in crate::component::home) fn device_exists(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<bool, PersistenceError> {
    devices::table
        .select(devices::device_id)
        .filter(devices::device_id.eq(device_id.as_text()))
        .first::<String>(transaction.connection())
        .optional()
        .map(|device_id| device_id.is_some())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::home) fn find_reset_epoch(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<Option<(String, Option<i64>)>, PersistenceError> {
    device_home_targets::table
        .select((
            device_home_targets::device_id,
            device_home_targets::reset_epoch,
        ))
        .filter(device_home_targets::device_id.eq(device_id.as_text()))
        .first::<(String, Option<i64>)>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::home) fn list_reset_epochs(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<(String, Option<i64>)>, PersistenceError> {
    device_home_targets::table
        .select((
            device_home_targets::device_id,
            device_home_targets::reset_epoch,
        ))
        .load(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::home) fn insert_target(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    reset_epoch: Option<i64>,
) -> Result<usize, PersistenceError> {
    diesel::insert_into(device_home_targets::table)
        .values((
            device_home_targets::device_id.eq(device_id.as_text()),
            device_home_targets::reset_epoch.eq(reset_epoch),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::home) fn set_reset_epoch(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    reset_epoch: i64,
) -> Result<usize, PersistenceError> {
    diesel::update(
        device_home_targets::table.filter(device_home_targets::device_id.eq(device_id.as_text())),
    )
    .set(device_home_targets::reset_epoch.eq(Some(reset_epoch)))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}
