use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};

use crate::{
    application::contest::{ContestPersistenceError, CurrentSeatProjection, SeatFacts},
    db::{Transaction, schema::seats},
};

pub(crate) fn list(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<SeatFacts>, ContestPersistenceError> {
    seats::table
        .select((seats::seat_id, seats::seat_code))
        .order(seats::seat_id)
        .load::<(String, String)>(transaction.connection())
        .map(|rows| {
            rows.into_iter()
                .map(|(seat_id, seat_code)| SeatFacts::new(seat_id, seat_code))
                .collect()
        })
        .map_err(|_| ContestPersistenceError::PersistenceFailed)
}

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    seat_id: &str,
    seat_code: &str,
) -> Result<usize, ContestPersistenceError> {
    diesel::insert_into(seats::table)
        .values((seats::seat_id.eq(seat_id), seats::seat_code.eq(seat_code)))
        .execute(transaction.connection())
        .map_err(|_| ContestPersistenceError::PersistenceFailed)
}

pub(crate) fn delete_exact(
    transaction: &mut Transaction<'_>,
    seat: &CurrentSeatProjection,
) -> Result<usize, ContestPersistenceError> {
    diesel::delete(
        seats::table
            .filter(seats::seat_id.eq(seat.seat_id()))
            .filter(seats::seat_code.eq(seat.seat_code())),
    )
    .execute(transaction.connection())
    .map_err(|_| ContestPersistenceError::PersistenceFailed)
}
