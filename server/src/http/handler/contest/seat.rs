use axum::{
    Extension, Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::component::{contest::SeatFacts, operator::OperatorIdentity};

use super::{super::super::error::ApiError, AppState, current_facts_response, middleware};

pub(super) fn routes(state: AppState) -> Router<AppState> {
    Router::new().route(
        "/seats",
        middleware::require_operator(state, get(list_seats)),
    )
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeatResponse {
    seat_id: String,
    seat_code: String,
}

impl From<SeatFacts> for SeatResponse {
    fn from(facts: SeatFacts) -> Self {
        let (seat_id, seat_code) = facts.into_parts();
        Self { seat_id, seat_code }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/seats",
    operation_id = "listSeats",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current Seat set", body = [SeatResponse]),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn list_seats(
    State(state): State<AppState>,
    Extension(_identity): Extension<OperatorIdentity>,
) -> Response {
    match state.contest().list_seats().await {
        Ok(facts) => {
            let response = facts
                .into_iter()
                .map(SeatResponse::from)
                .collect::<Vec<_>>();
            current_facts_response(&response)
        }
        Err(error) => ApiError::from_contest(error).into_response(),
    }
}
