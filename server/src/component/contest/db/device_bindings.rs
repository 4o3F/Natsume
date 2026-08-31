use diesel::{QueryDsl, RunQueryDsl};

use crate::{
    component::{
        contest::{BindingFacts, ContestPersistenceError},
        lifecycle::DeviceId,
    },
    db::Transaction,
    diesel_schema::device_bindings,
};

pub(in crate::component::contest) fn list(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<BindingFacts>, ContestPersistenceError> {
    let rows = device_bindings::table
        .select((
            device_bindings::binding_id,
            device_bindings::seat_id,
            device_bindings::device_id,
        ))
        .order(device_bindings::seat_id)
        .load::<(String, String, String)>(transaction.connection())
        .map_err(|_| ContestPersistenceError::PersistenceFailed)?;
    rows.into_iter()
        .map(|(binding_id, seat_id, device_id)| {
            let device_id = DeviceId::parse(&device_id)
                .ok_or(ContestPersistenceError::InvalidPersistedFacts)?;
            Ok(BindingFacts::new(binding_id, seat_id, device_id))
        })
        .collect()
}
