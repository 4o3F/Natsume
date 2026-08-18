use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl};
use uuid::Uuid;

use crate::{
    application::device::{
        DeviceFacts, DeviceId, DevicePersistenceError, DeviceState, HardwareIdentityQuality,
        enrollment::ValidatedEnrollmentRequest,
    },
    db::{Transaction, schema::devices},
};

pub(crate) fn find_device_pk(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<Option<DeviceId>, DevicePersistenceError> {
    let persisted_device_id = devices::table
        .select(devices::device_pk)
        .filter(devices::device_pk.eq(device_id.as_text()))
        .first::<String>(transaction.connection())
        .optional()
        .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    persisted_device_id
        .map(|device_id| {
            DeviceId::parse(&device_id).ok_or(DevicePersistenceError::InvalidPersistedFacts)
        })
        .transpose()
}

pub(crate) fn list(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<DeviceFacts>, DevicePersistenceError> {
    let rows = devices::table
        .select((
            devices::device_pk,
            devices::state,
            devices::hardware_identity_quality,
        ))
        .order(devices::device_pk)
        .load::<(String, String, String)>(transaction.connection())
        .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    rows.into_iter()
        .map(|(device_id, state, hardware_identity_quality)| {
            let device_id =
                DeviceId::parse(&device_id).ok_or(DevicePersistenceError::InvalidPersistedFacts)?;
            let state = DeviceState::from_persisted(&state)
                .ok_or(DevicePersistenceError::InvalidPersistedFacts)?;
            let hardware_identity_quality =
                HardwareIdentityQuality::parse(&hardware_identity_quality)
                    .ok_or(DevicePersistenceError::InvalidPersistedFacts)?;
            Ok(DeviceFacts::new(
                device_id,
                state,
                hardware_identity_quality,
            ))
        })
        .collect()
}

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    device_id: Uuid,
    request: &ValidatedEnrollmentRequest,
) -> Result<(), DevicePersistenceError> {
    diesel::insert_into(devices::table)
        .values((
            devices::device_pk.eq(device_id.to_string()),
            devices::machine_hardware_id.eq(&request.machine_hardware_id),
            devices::hardware_identity_quality.eq(request.hardware_identity_quality.as_persisted()),
            devices::state.eq(DeviceState::Enrolled.as_persisted()),
        ))
        .execute(transaction.connection())
        .map(|_| ())
        .map_err(|_| DevicePersistenceError::PersistenceFailed)
}

pub(crate) fn restore_enrolled(
    transaction: &mut Transaction<'_>,
    device_id: Uuid,
    expected_state: DeviceState,
) -> Result<(), DevicePersistenceError> {
    let updated = diesel::update(
        devices::table
            .filter(devices::device_pk.eq(device_id.to_string()))
            .filter(devices::state.eq(expected_state.as_persisted())),
    )
    .set(devices::state.eq(DeviceState::Enrolled.as_persisted()))
    .execute(transaction.connection())
    .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    if updated != 1 {
        return Err(DevicePersistenceError::PersistenceFailed);
    }
    Ok(())
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
            .filter(devices::device_pk.eq(device_id))
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
