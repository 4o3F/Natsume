use axum::{
    Extension, Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::component::{contest::AccountFacts, operator::OperatorIdentity};

use super::{super::super::error::ApiError, AppState, current_facts_response, middleware};

pub(super) fn routes(state: AppState) -> Router<AppState> {
    Router::new().route(
        "/accounts",
        middleware::require_operator(state, get(list_accounts)),
    )
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountResponse {
    account_id: String,
    domjudge_username: String,
    credential_revision: i64,
}

impl From<AccountFacts> for AccountResponse {
    fn from(facts: AccountFacts) -> Self {
        let (account_id, domjudge_username, credential_revision) = facts.into_parts();
        Self {
            account_id,
            domjudge_username,
            credential_revision,
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/accounts",
    operation_id = "listAccounts",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current Account set", body = [AccountResponse]),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn list_accounts(
    State(state): State<AppState>,
    Extension(_identity): Extension<OperatorIdentity>,
) -> Response {
    match state.contest().list_accounts().await {
        Ok(facts) => {
            let response = facts
                .into_iter()
                .map(AccountResponse::from)
                .collect::<Vec<_>>();
            current_facts_response(&response)
        }
        Err(error) => ApiError::from_contest(error).into_response(),
    }
}
