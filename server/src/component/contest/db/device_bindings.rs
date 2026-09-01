use diesel::{QueryDsl, RunQueryDsl};

use crate::{
    component::{contest::BindingFacts, device::DeviceId},
    db::{PersistenceError, Transaction},
    diesel_schema::device_bindings,
};

pub(in crate::component::contest) fn list(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<BindingFacts>, PersistenceError> {
    let rows = device_bindings::table
        .select((
            device_bindings::binding_id,
            device_bindings::seat_id,
            device_bindings::device_id,
        ))
        .order(device_bindings::seat_id)
        .load::<(String, String, String)>(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?;
    rows.into_iter()
        .map(|(binding_id, seat_id, device_id)| {
            let device_id =
                DeviceId::parse(&device_id).ok_or(PersistenceError::InvalidPersistedData)?;
            Ok(BindingFacts::new(binding_id, seat_id, device_id))
        })
        .collect()
}
