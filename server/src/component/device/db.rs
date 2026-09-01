#![allow(dead_code)]

use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, dsl::sql, sql_types::BigInt,
};

use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::devices,
};

use super::types::{DeviceFacts, DeviceId, DeviceState, EvidenceQuality};

fn find_state(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<Option<DeviceState>, PersistenceError> {
    let persisted_state = devices::table
        .select(devices::state)
        .filter(devices::device_id.eq(device_id.as_text()))
        .first::<String>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?;
    persisted_state
        .map(|state| {
            DeviceState::from_persisted(&state).ok_or(PersistenceError::InvalidPersistedData)
        })
        .transpose()
}

fn list(transaction: &mut Transaction<'_>) -> Result<Vec<DeviceFacts>, PersistenceError> {
    let rows = devices::table
        .select((
            devices::device_id,
            devices::state,
            devices::evidence_quality,
        ))
        .order(devices::device_id)
        .load::<(String, String, String)>(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?;
    rows.into_iter()
        .map(|(device_id, state, evidence_quality)| {
            let device_id =
                DeviceId::parse(&device_id).ok_or(PersistenceError::InvalidPersistedData)?;
            let state = DeviceState::from_persisted(&state)
                .ok_or(PersistenceError::InvalidPersistedData)?;
            let evidence_quality = EvidenceQuality::parse(&evidence_quality)
                .ok_or(PersistenceError::InvalidPersistedData)?;
            Ok(DeviceFacts::new(device_id, state, evidence_quality))
        })
        .collect()
}

fn insert(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    machine_hardware_id: &str,
    evidence_quality: EvidenceQuality,
) -> Result<(), PersistenceError> {
    diesel::insert_into(devices::table)
        .values((
            devices::device_id.eq(device_id.as_text()),
            devices::machine_hardware_id.eq(machine_hardware_id),
            devices::evidence_quality.eq(evidence_quality.as_persisted()),
            devices::state.eq(DeviceState::Enabled.as_persisted()),
            devices::created_at_unix_ms
                .eq(sql::<BigInt>("CAST(unixepoch('subsec') * 1000 AS INTEGER)")),
        ))
        .execute(transaction.connection())
        .map(|_| ())
        .map_err(|_| PersistenceError::OperationFailed)
}

fn update_state_guarded(
    transaction: &mut Transaction<'_>,
    device_id: &str,
    expected: DeviceState,
    next: DeviceState,
) -> Result<(), PersistenceError> {
    if expected == next {
        return Ok(());
    }
    let updated = diesel::update(
        devices::table
            .filter(devices::device_id.eq(device_id))
            .filter(devices::state.eq(expected.as_persisted())),
    )
    .set(devices::state.eq(next.as_persisted()))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)?;
    if updated != 1 {
        return Err(PersistenceError::InvalidPersistedData);
    }
    Ok(())
}
