use crate::{
    component::device::DeviceId,
    db::{PersistenceError, Transaction},
    diesel_schema::{device_power_targets, devices},
};

type PersistedTarget = (Option<i64>, Option<i64>);
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl};

pub(in crate::component::power) fn device_exists(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<bool, PersistenceError> {
    devices::table
        .select(devices::device_id)
        .filter(devices::device_id.eq(device_id.as_text()))
        .first::<String>(transaction.connection())
        .optional()
        .map(|value| value.is_some())
        .map_err(|_| PersistenceError::OperationFailed)
}
pub(in crate::component::power) fn find(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<Option<PersistedTarget>, PersistenceError> {
    device_power_targets::table
        .select((
            device_power_targets::shutdown_epoch,
            device_power_targets::expires_at_unix_ms,
        ))
        .filter(device_power_targets::device_id.eq(device_id.as_text()))
        .first(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)
}
pub(in crate::component::power) fn insert(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<usize, PersistenceError> {
    diesel::insert_into(device_power_targets::table)
        .values(device_power_targets::device_id.eq(device_id.as_text()))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}
pub(in crate::component::power) fn update(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    epoch: i64,
    deadline: i64,
) -> Result<usize, PersistenceError> {
    diesel::update(
        device_power_targets::table.filter(device_power_targets::device_id.eq(device_id.as_text())),
    )
    .set((
        device_power_targets::shutdown_epoch.eq(Some(epoch)),
        device_power_targets::expires_at_unix_ms.eq(Some(deadline)),
    ))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}
