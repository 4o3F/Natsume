use std::sync::Arc;

use crate::http::Routes;

use axum::{
    Extension, Json,
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

use crate::component::{
    contest::{BindingFacts, ContestComponent},
    operator::OperatorIdentity,
};

use super::super::super::error::ApiError;

pub(super) fn operator_routes() -> Routes<Arc<ContestComponent>> {
    vec![("/bindings", get(list_bindings))]
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)] // The API fields are canonical domain identifiers.
pub(crate) struct BindingResponse {
    binding_id: String,
    seat_id: String,
    device_id: String,
}

impl From<BindingFacts> for BindingResponse {
    fn from(facts: BindingFacts) -> Self {
        let BindingFacts {
            binding: binding_id,
            seat: seat_id,
            device: device_id,
        } = facts;
        Self {
            binding_id,
            seat_id,
            device_id: device_id.as_text(),
        }
    }
}

pub(crate) async fn list_bindings(
    State(state): State<Arc<ContestComponent>>,
    Extension(_identity): Extension<OperatorIdentity>,
) -> Response {
    match state.list_bindings().await {
        Ok(facts) => {
            let response = facts
                .into_iter()
                .map(BindingResponse::from)
                .collect::<Vec<_>>();
            Json(response).into_response()
        }
        Err(error) => ApiError::from_contest(error).into_response(),
    }
}
