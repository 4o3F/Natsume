use axum::{
    Extension, Router,
    extract::State,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    audit::CorrelationId,
    component::{contest::AccountFacts, operator::OperatorIdentity},
};

use super::{super::super::error::ApiError, AppState, current_facts_response, middleware};

pub(super) fn routes(state: AppState) -> Router<AppState> {
    Router::new().route("/accounts", middleware::operator_get(state, list_accounts))
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
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(_identity): Extension<OperatorIdentity>,
) -> Response {
    match state.contest().list_accounts().await {
        Ok(facts) => {
            let response = facts
                .into_iter()
                .map(AccountResponse::from)
                .collect::<Vec<_>>();
            current_facts_response(&response, correlation_id)
        }
        Err(error) => ApiError::from_contest(error, correlation_id).into_response(),
    }
}
