use axum::{
    Extension, Router,
    extract::State,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    audit::CorrelationId,
    component::{contest::BindingFacts, operator::OperatorIdentity},
};

use super::{super::super::error::ApiError, AppState, current_facts_response, middleware};

pub(super) fn routes(state: AppState) -> Router<AppState> {
    Router::new().route("/bindings", middleware::operator_get(state, list_bindings))
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)] // The API fields are canonical domain identifiers.
pub(crate) struct BindingResponse {
    binding_id: String,
    seat_id: String,
    device_id: String,
}

impl From<BindingFacts> for BindingResponse {
    fn from(facts: BindingFacts) -> Self {
        let (binding_id, seat_id, device_id) = facts.into_parts();
        Self {
            binding_id,
            seat_id,
            device_id: device_id.as_text(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/bindings",
    operation_id = "listBindings",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current Seat-to-Device Binding set", body = [BindingResponse]),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn list_bindings(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(_identity): Extension<OperatorIdentity>,
) -> Response {
    match state.contest().list_bindings().await {
        Ok(facts) => {
            let response = facts
                .into_iter()
                .map(BindingResponse::from)
                .collect::<Vec<_>>();
            current_facts_response(&response, correlation_id)
        }
        Err(error) => ApiError::from_contest(error, correlation_id).into_response(),
    }
}
