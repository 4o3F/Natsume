use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};

use crate::{
    application::{
        contest::{ContestError, SeatFacts},
        import::{CurrentSeatProjection, ImportError},
    },
    db::{Transaction, schema::seats},
};

pub(crate) fn list(transaction: &mut Transaction<'_>) -> Result<Vec<SeatFacts>, ContestError> {
    seats::table
        .select((seats::seat_id, seats::seat_code))
        .order(seats::seat_id)
        .load::<(String, String)>(transaction.connection())
        .map(|rows| {
            rows.into_iter()
                .map(|(seat_id, seat_code)| SeatFacts::new(seat_id, seat_code))
                .collect()
        })
        .map_err(|_| ContestError::PersistenceFailed)
}

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    seat_id: &str,
    seat_code: &str,
) -> Result<usize, ImportError> {
    diesel::insert_into(seats::table)
        .values((seats::seat_id.eq(seat_id), seats::seat_code.eq(seat_code)))
        .execute(transaction.connection())
        .map_err(|_| ImportError::PersistenceFailure)
}

pub(crate) fn delete_exact(
    transaction: &mut Transaction<'_>,
    seat: &CurrentSeatProjection,
) -> Result<usize, ImportError> {
    diesel::delete(
        seats::table
            .filter(seats::seat_id.eq(seat.seat_id()))
            .filter(seats::seat_code.eq(seat.seat_code())),
    )
    .execute(transaction.connection())
    .map_err(|_| ImportError::PersistenceFailure)
}
