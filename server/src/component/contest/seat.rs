use crate::{component::lifecycle::DeviceId, db::Database};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentSeatProjection {
    seat_id: String,
    seat_code: String,
    current_domjudge_username: Option<String>,
    device_id: Option<DeviceId>,
}

impl CurrentSeatProjection {
    pub(crate) fn new(
        seat_id: String,
        seat_code: String,
        current_domjudge_username: Option<String>,
        device_id: Option<DeviceId>,
    ) -> Self {
        Self {
            seat_id,
            seat_code,
            current_domjudge_username,
            device_id,
        }
    }

    #[must_use]
    pub(crate) fn seat_id(&self) -> &str {
        &self.seat_id
    }

    #[must_use]
    pub(crate) fn seat_code(&self) -> &str {
        &self.seat_code
    }

    #[must_use]
    pub(crate) fn current_domjudge_username(&self) -> Option<&str> {
        self.current_domjudge_username.as_deref()
    }

    #[must_use]
    pub(crate) const fn device_id(&self) -> Option<&DeviceId> {
        self.device_id.as_ref()
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
        .map_err(ContestError::from_persistence)
}
