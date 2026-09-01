use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};

use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::seats,
};

use super::super::baseline::BaselineSeat;

pub(in crate::component::import) fn insert(
    transaction: &mut Transaction<'_>,
    seat_id: &str,
    seat_code: &str,
) -> Result<usize, PersistenceError> {
    diesel::insert_into(seats::table)
        .values((seats::seat_id.eq(seat_id), seats::seat_code.eq(seat_code)))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::import) fn delete_exact(
    transaction: &mut Transaction<'_>,
    seat: &BaselineSeat,
) -> Result<usize, PersistenceError> {
    diesel::delete(
        seats::table
            .filter(seats::seat_id.eq(seat.seat_id()))
            .filter(seats::seat_code.eq(seat.seat_code())),
    )
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}
