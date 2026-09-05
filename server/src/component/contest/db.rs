use diesel::{QueryDsl, RunQueryDsl, dsl::sql, sql_types::BigInt};

use crate::{
    component::{
        contest::{AccountFacts, BindingFacts, SeatFacts},
        device::DeviceId,
    },
    db::{PersistenceError, Transaction},
    diesel_schema::{accounts, device_bindings, seats},
};

pub(in crate::component::contest) fn list_seats(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<SeatFacts>, PersistenceError> {
    seats::table
        .select((seats::seat_id, seats::seat_code))
        .order(seats::seat_id)
        .load::<(String, String)>(transaction.connection())
        .map(|rows| {
            rows.into_iter()
                .map(|(seat_id, seat_code)| SeatFacts { seat_id, seat_code })
                .collect()
        })
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::contest) fn list_accounts(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<AccountFacts>, PersistenceError> {
    accounts::table
        .select((
            accounts::account_id,
            accounts::domjudge_username,
            sql::<BigInt>("credential_revision"),
        ))
        .order(accounts::account_id)
        .load::<(String, String, i64)>(transaction.connection())
        .map(|rows| {
            rows.into_iter()
                .map(
                    |(account_id, domjudge_username, credential_revision)| AccountFacts {
                        account_id,
                        domjudge_username,
                        credential_revision,
                    },
                )
                .collect()
        })
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::contest) fn list_bindings(
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
            Ok(BindingFacts {
                binding: binding_id,
                seat: seat_id,
                device: device_id,
            })
        })
        .collect()
}
