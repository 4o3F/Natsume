use diesel::{QueryDsl, RunQueryDsl};

use crate::{
    component::contest::SeatFacts,
    db::{PersistenceError, Transaction},
    diesel_schema::seats,
};

pub(in crate::component::contest) fn list(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<SeatFacts>, PersistenceError> {
    seats::table
        .select((seats::seat_id, seats::seat_code))
        .order(seats::seat_id)
        .load::<(String, String)>(transaction.connection())
        .map(|rows| {
            rows.into_iter()
                .map(|(seat_id, seat_code)| SeatFacts::new(seat_id, seat_code))
                .collect()
        })
        .map_err(|_| PersistenceError::OperationFailed)
}
