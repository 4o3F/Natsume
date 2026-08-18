use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl, dsl::sql, sql_types::BigInt};

use crate::{
    application::{
        contest::{BindingFacts, ContestPersistenceError},
        device::DeviceId,
    },
    db::{Transaction, schema::device_bindings},
};

pub(crate) fn list(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<BindingFacts>, ContestPersistenceError> {
    let rows = device_bindings::table
        .select((
            device_bindings::seat_id,
            device_bindings::device_pk,
            sql::<BigInt>("binding_revision"),
        ))
        .order(device_bindings::seat_id)
        .load::<(String, String, i64)>(transaction.connection())
        .map_err(|_| ContestPersistenceError::PersistenceFailed)?;
    rows.into_iter()
        .map(|(seat_id, device_id, binding_revision)| {
            let device_id = DeviceId::parse(&device_id)
                .ok_or(ContestPersistenceError::InvalidPersistedFacts)?;
            Ok(BindingFacts::new(seat_id, device_id, binding_revision))
        })
        .collect()
}

pub(crate) fn delete_by_seat(
    transaction: &mut Transaction<'_>,
    seat_id: &str,
) -> Result<usize, ContestPersistenceError> {
    diesel::delete(device_bindings::table.filter(device_bindings::seat_id.eq(seat_id)))
        .execute(transaction.connection())
        .map_err(|_| ContestPersistenceError::PersistenceFailed)
}
