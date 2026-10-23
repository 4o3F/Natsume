use axum::{
    Extension, Json, Router,
    extract::{Request, State, rejection::JsonRejection},
    http::header,
    middleware::{self as axum_middleware, Next},
    response::{IntoResponse as _, Response},
    routing::{MethodRouter, get, post, put},
};

use super::{ApiError, AppState, PrintError, PrintPathId, middleware};
use crate::component::print::{
    ClientIdentity,
    execution::{AttemptReport, JobIdentity, PrintClaim},
};

pub(super) fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/print/client/claims",
            require_client(state.clone(), post(claim)),
        )
        .route(
            "/print/client/attempts/current",
            require_client(state.clone(), get(current)),
        )
        .route(
            "/print/client/attempts/{attempt_id}",
            require_client(state.clone(), get(attempt)),
        )
        .route(
            "/print/client/attempts/{attempt_id}/job",
            require_client(state.clone(), put(register_job)),
        )
        .route(
            "/print/client/attempts/{attempt_id}/reports",
            require_client(state.clone(), post(report)),
        )
        .route(
            "/print/client/attempts/{attempt_id}/document",
            require_client(state.clone(), get(document)),
        )
        .route(
            "/print/executions",
            middleware::require_operator(state, get(executions)),
        )
}

fn require_client(state: AppState, routes: MethodRouter<AppState>) -> MethodRouter<AppState> {
    routes.route_layer(axum_middleware::from_fn_with_state(state, authenticate))
}

async fn authenticate(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let mut values = request.headers().get_all(header::AUTHORIZATION).iter();
    let token = values
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) = token.filter(|_| values.next().is_none()) else {
        return ApiError::from_print(PrintError::Authentication).into_response();
    };
    match state.print().authenticate_client(token).await {
        Ok(client) => {
            request.extensions_mut().insert(client);
            next.run(request).await
        }
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn claim(
    State(state): State<AppState>,
    Extension(client): Extension<ClientIdentity>,
    body: Result<Json<PrintClaim>, JsonRejection>,
) -> Response {
    let Ok(Json(input)) = body else {
        return ApiError::invalid_request("print_claim_json").into_response();
    };
    match state.print().claim(client, input).await {
        Ok(attempt) => Json(attempt).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn current(
    State(state): State<AppState>,
    Extension(client): Extension<ClientIdentity>,
) -> Response {
    match state.print().current_attempt(client).await {
        Ok(attempt) => Json(attempt).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn attempt(
    State(state): State<AppState>,
    Extension(client): Extension<ClientIdentity>,
    PrintPathId(id): PrintPathId,
) -> Response {
    match state.print().get_attempt(client, id).await {
        Ok(attempt) => Json(attempt).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn register_job(
    State(state): State<AppState>,
    Extension(client): Extension<ClientIdentity>,
    PrintPathId(id): PrintPathId,
    body: Result<Json<JobIdentity>, JsonRejection>,
) -> Response {
    let Ok(Json(job)) = body else {
        return ApiError::invalid_request("print_job_json").into_response();
    };
    match state.print().register_job(client, id, job).await {
        Ok(attempt) => Json(attempt).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn report(
    State(state): State<AppState>,
    Extension(client): Extension<ClientIdentity>,
    PrintPathId(id): PrintPathId,
    body: Result<Json<AttemptReport>, JsonRejection>,
) -> Response {
    let Ok(Json(report)) = body else {
        return ApiError::invalid_request("print_report_json").into_response();
    };
    match state.print().report_attempt(client, id, report).await {
        Ok(attempt) => Json(attempt).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn document(
    State(state): State<AppState>,
    Extension(client): Extension<ClientIdentity>,
    PrintPathId(id): PrintPathId,
) -> Response {
    match state.print().attempt_document(client, id).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/pdf")], bytes).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn executions(State(state): State<AppState>) -> Response {
    match state.print().list_executions().await {
        Ok(executions) => Json(executions).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}
