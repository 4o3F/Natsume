use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl};

use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::device_control_keys,
};

use super::super::types::{ControlPublicKey, DeviceId};

const CURRENT: &str = "current";
const RETIRED: &str = "retired";

pub(in crate::component::device) fn public_key_exists(
    transaction: &mut Transaction<'_>,
    public_key: &ControlPublicKey,
) -> Result<bool, PersistenceError> {
    device_control_keys::table
        .select(device_control_keys::public_key)
        .filter(device_control_keys::public_key.eq(public_key.as_bytes().as_slice()))
        .first::<Vec<u8>>(transaction.connection())
        .optional()
        .map(|row| row.is_some())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::device) fn find_current_for_device(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<Option<ControlPublicKey>, PersistenceError> {
    device_control_keys::table
        .select(device_control_keys::public_key)
        .filter(device_control_keys::device_id.eq(device_id.as_text()))
        .filter(device_control_keys::status.eq(CURRENT))
        .filter(device_control_keys::retired_at_unix_ms.is_null())
        .first::<Vec<u8>>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
        .map(|public_key| {
            ControlPublicKey::parse(&public_key).ok_or(PersistenceError::InvalidPersistedData)
        })
        .transpose()
}

pub(in crate::component::device) fn insert_current(
    transaction: &mut Transaction<'_>,
    public_key: &ControlPublicKey,
    device_id: &DeviceId,
    activated_at_unix_ms: i64,
) -> Result<usize, PersistenceError> {
    diesel::insert_into(device_control_keys::table)
        .values((
            device_control_keys::public_key.eq(public_key.as_bytes().as_slice()),
            device_control_keys::device_id.eq(device_id.as_text()),
            device_control_keys::status.eq(CURRENT),
            device_control_keys::activated_at_unix_ms.eq(activated_at_unix_ms),
            device_control_keys::retired_at_unix_ms.eq(None::<i64>),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::device) fn retire_current(
    transaction: &mut Transaction<'_>,
    public_key: &ControlPublicKey,
    device_id: &DeviceId,
    retired_at_unix_ms: i64,
) -> Result<usize, PersistenceError> {
    diesel::update(
        device_control_keys::table
            .filter(device_control_keys::public_key.eq(public_key.as_bytes().as_slice()))
            .filter(device_control_keys::device_id.eq(device_id.as_text()))
            .filter(device_control_keys::status.eq(CURRENT))
            .filter(device_control_keys::retired_at_unix_ms.is_null()),
    )
    .set((
        device_control_keys::status.eq(RETIRED),
        device_control_keys::retired_at_unix_ms.eq(Some(retired_at_unix_ms)),
    ))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}
