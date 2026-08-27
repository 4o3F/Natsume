use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};

use crate::{
    component::{contest::CurrentSeatProjection, import::ImportError},
    db::Transaction,
    schema::seats,
};

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
