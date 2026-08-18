use crate::db::{self, Database};

use super::ContestError;

pub(crate) struct SeatFacts {
    seat_id: String,
    seat_code: String,
}

impl SeatFacts {
    pub(crate) fn new(seat_id: String, seat_code: String) -> Self {
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
pub(crate) async fn list_seats(database: &Database) -> Result<Vec<SeatFacts>, ContestError> {
    database.read(db::contest::seats::list).await
}
