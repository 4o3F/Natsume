use crate::http::Routes;

use axum::{
    Extension, Json,
    extract::{
        Path, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{StatusCode, header},
    response::{IntoResponse as _, Response},
    routing::{get, post, put},
};

use super::{ApiError, AppState, PrintPathId, middleware};
use crate::component::print::{
    ClientIdentity,
    execution::{AttemptReport, JobIdentity, PrintClaim, PrintRequeue},
};

pub(super) fn client_routes() -> Routes {
    vec![
        ("/print/client/claims", post(claim)),
        ("/print/client/attempts/current", get(current)),
        ("/print/client/attempts/{attempt_id}", get(attempt)),
        ("/print/client/attempts/{attempt_id}/job", put(register_job)),
        ("/print/client/attempts/{attempt_id}/reports", post(report)),
        (
            "/print/client/attempts/{attempt_id}/document",
            get(document),
        ),
    ]
}

pub(super) fn operator_routes() -> Routes {
    vec![
        (
            "/print/executions/{ready_sequence}/requeue",
            middleware::require_admin(post(requeue)),
        ),
        ("/print/executions", get(executions)),
    ]
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

async fn requeue(
    State(state): State<AppState>,
    path: Result<Path<i64>, PathRejection>,
    body: Result<Json<PrintRequeue>, JsonRejection>,
) -> Response {
    let Ok(Path(sequence)) = path else {
        return ApiError::invalid_request("print_execution_sequence").into_response();
    };
    let Ok(Json(input)) = body else {
        return ApiError::invalid_request("print_requeue_json").into_response();
    };
    match state.print().requeue(sequence, input).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn executions(State(state): State<AppState>) -> Response {
    match state.print().list_executions().await {
        Ok(executions) => Json(executions).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}
