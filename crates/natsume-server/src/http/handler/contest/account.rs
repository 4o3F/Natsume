use axum::{
    Extension, Json, Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::component::{contest::AccountFacts, operator::OperatorIdentity};

use super::{super::super::error::ApiError, AppState, middleware};

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
    team: Option<TeamResponse>,
}

impl From<AccountFacts> for AccountResponse {
    fn from(facts: AccountFacts) -> Self {
        let AccountFacts {
            account_id,
            domjudge_username,
            credential_revision,
            team,
        } = facts;
        Self {
            account_id,
            domjudge_username,
            credential_revision,
            team: team.map(TeamResponse::from),
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TeamResponse {
    seat_id: String,
    seat_code: String,
    organization_id: String,
    team_name_zh: String,
    team_name_en: String,
    school_name_zh: String,
    school_name_en: String,
}

impl From<crate::component::contest::TeamDetails> for TeamResponse {
    fn from(team: crate::component::contest::TeamDetails) -> Self {
        Self {
            seat_id: team.seat_id,
            seat_code: team.seat_code,
            organization_id: format!("INST-{:03}", team.organization_id),
            team_name_zh: team.team_name_zh,
            team_name_en: team.team_name_en,
            school_name_zh: team.school_name_zh,
            school_name_en: team.school_name_en,
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
            Json(response).into_response()
        }
        Err(error) => ApiError::from_contest(error).into_response(),
    }
}
