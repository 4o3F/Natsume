use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, dsl::exists};

use crate::{
    component::device::DeviceId,
    db::{PersistenceError, Transaction},
    diesel_schema::{device_session_targets, devices},
};

pub(in crate::component::session) fn device_exists(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<bool, PersistenceError> {
    diesel::select(exists(
        devices::table.filter(devices::device_id.eq(device_id.as_text())),
    ))
    .get_result::<bool>(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::session) fn find_target(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<Option<(String, Option<i64>)>, PersistenceError> {
    device_session_targets::table
        .select((
            device_session_targets::lock_state,
            device_session_targets::terminate_epoch,
        ))
        .filter(device_session_targets::device_id.eq(device_id.as_text()))
        .first::<(String, Option<i64>)>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::session) fn list_targets(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<(String, String, Option<i64>)>, PersistenceError> {
    device_session_targets::table
        .select((
            device_session_targets::device_id,
            device_session_targets::lock_state,
            device_session_targets::terminate_epoch,
        ))
        .load(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::session) fn insert_default_target(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<usize, PersistenceError> {
    diesel::insert_into(device_session_targets::table)
        .values((
            device_session_targets::device_id.eq(device_id.as_text()),
            device_session_targets::lock_state.eq("unlocked"),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::session) fn update_lock_state(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    lock_state: &str,
) -> Result<usize, PersistenceError> {
    diesel::update(
        device_session_targets::table
            .filter(device_session_targets::device_id.eq(device_id.as_text())),
    )
    .set(device_session_targets::lock_state.eq(lock_state))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::session) fn update_terminate_epoch(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    terminate_epoch: i64,
) -> Result<usize, PersistenceError> {
    if terminate_epoch < 1 {
        return Err(PersistenceError::InvalidPersistedData);
    }
    diesel::update(
        device_session_targets::table
            .filter(device_session_targets::device_id.eq(device_id.as_text())),
    )
    .set(device_session_targets::terminate_epoch.eq(Some(terminate_epoch)))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}
