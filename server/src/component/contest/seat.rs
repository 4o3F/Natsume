use crate::db::{Database, TransactionError};

use super::ContestError;

pub(crate) struct SeatFacts {
    seat_id: String,
    seat_code: String,
}

impl SeatFacts {
    pub(super) fn new(seat_id: String, seat_code: String) -> Self {
        Self { seat_id, seat_code }
    }

    pub(crate) fn into_parts(self) -> (String, String) {
        (self.seat_id, self.seat_code)
    }
}

/// Reads the current Seat set in deterministic natural-key order.
///
/// # Errors
///
/// Returns a redacted [`ContestError`] when persistence fails.
pub(super) async fn list_seats(database: &Database) -> Result<Vec<SeatFacts>, ContestError> {
    database
        .read(crate::component::contest::db::seats::list)
        .await
        .map_err(TransactionError::into_error)
        .map_err(ContestError::from)
}
