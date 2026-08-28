#![allow(dead_code)]

use crate::{
    component::lifecycle::{
        DeviceFacts, DeviceId, DevicePersistenceError, DeviceState, EvidenceQuality,
    },
    db::Transaction,
    diesel_schema::devices,
};
use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, dsl::sql, sql_types::BigInt,
};

pub(crate) fn find_state(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<Option<DeviceState>, DevicePersistenceError> {
    let persisted_state = devices::table
        .select(devices::state)
        .filter(devices::device_id.eq(device_id.as_text()))
        .first::<String>(transaction.connection())
        .optional()
        .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    persisted_state
        .map(|state| {
            DeviceState::from_persisted(&state).ok_or(DevicePersistenceError::InvalidPersistedFacts)
        })
        .transpose()
}

pub(crate) fn list(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<DeviceFacts>, DevicePersistenceError> {
    let rows = devices::table
        .select((
            devices::device_id,
            devices::state,
            devices::evidence_quality,
        ))
        .order(devices::device_id)
        .load::<(String, String, String)>(transaction.connection())
        .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    rows.into_iter()
        .map(|(device_id, state, evidence_quality)| {
            let device_id =
                DeviceId::parse(&device_id).ok_or(DevicePersistenceError::InvalidPersistedFacts)?;
            let state = DeviceState::from_persisted(&state)
                .ok_or(DevicePersistenceError::InvalidPersistedFacts)?;
            let evidence_quality = EvidenceQuality::parse(&evidence_quality)
                .ok_or(DevicePersistenceError::InvalidPersistedFacts)?;
            Ok(DeviceFacts::new(device_id, state, evidence_quality))
        })
        .collect()
}

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    machine_hardware_id: &str,
    evidence_quality: EvidenceQuality,
) -> Result<(), DevicePersistenceError> {
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
        .map_err(|_| DevicePersistenceError::PersistenceFailed)
}

pub(crate) fn update_state_guarded(
    transaction: &mut Transaction<'_>,
    device_id: &str,
    expected: DeviceState,
    next: DeviceState,
) -> Result<(), DevicePersistenceError> {
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
    .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    if updated != 1 {
        return Err(DevicePersistenceError::PersistenceFailed);
    }
    Ok(())
}
