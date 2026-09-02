use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl};

use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::devices,
};

use super::super::types::{
    DeviceId, DeviceRecord, DeviceState, EvidenceQuality, MachineHardwareId,
};

type PersistedDevice = (String, String);

pub(in crate::component::device) fn find_by_id(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<Option<DeviceRecord>, PersistenceError> {
    devices::table
        .select((devices::device_id, devices::state))
        .filter(devices::device_id.eq(device_id.as_text()))
        .first::<PersistedDevice>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
        .map(|row| parse(&row))
        .transpose()
}

pub(in crate::component::device) fn find_non_revoked_by_machine(
    transaction: &mut Transaction<'_>,
    machine_hardware_id: &MachineHardwareId,
) -> Result<Option<DeviceRecord>, PersistenceError> {
    devices::table
        .select((devices::device_id, devices::state))
        .filter(devices::machine_hardware_id.eq(machine_hardware_id.as_text()))
        .filter(devices::state.ne(DeviceState::Revoked.as_persisted()))
        .first::<PersistedDevice>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
        .map(|row| parse(&row))
        .transpose()
}

pub(in crate::component::device) fn insert(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    machine_hardware_id: &MachineHardwareId,
    evidence_quality: EvidenceQuality,
    created_at_unix_ms: i64,
) -> Result<usize, PersistenceError> {
    diesel::insert_into(devices::table)
        .values((
            devices::device_id.eq(device_id.as_text()),
            devices::machine_hardware_id.eq(machine_hardware_id.as_text()),
            devices::evidence_quality.eq(evidence_quality.as_persisted()),
            devices::state.eq(DeviceState::Enabled.as_persisted()),
            devices::created_at_unix_ms.eq(created_at_unix_ms),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::device) fn update_evidence(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    next: EvidenceQuality,
) -> Result<usize, PersistenceError> {
    diesel::update(devices::table.filter(devices::device_id.eq(device_id.as_text())))
        .set(devices::evidence_quality.eq(next.as_persisted()))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::device) fn update_state(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    next: DeviceState,
) -> Result<usize, PersistenceError> {
    diesel::update(devices::table.filter(devices::device_id.eq(device_id.as_text())))
        .set(devices::state.eq(next.as_persisted()))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

fn parse(row: &PersistedDevice) -> Result<DeviceRecord, PersistenceError> {
    DeviceRecord::from_persisted(&row.0, &row.1)
}
