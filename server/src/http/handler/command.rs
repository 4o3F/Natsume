use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State, rejection::JsonRejection},
    http::StatusCode,
    middleware as axum_middleware,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::put,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use utoipa::{IntoParams, ToSchema};

use crate::{
    application::{
        command::{self, CommandError, CommandId, CommandOutcome, CommandRequestInput},
        operator::{self, OperatorIdentity},
    },
    audit::CorrelationId,
};

use super::super::{AppState, error::ApiError, middleware};

pub(crate) const COMMAND_REQUEST_BODY_LIMIT_BYTES: usize = 16_384;

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    let command = put(put_command)
        .layer(DefaultBodyLimit::max(COMMAND_REQUEST_BODY_LIMIT_BYTES))
        .route_layer(axum_middleware::from_fn(require_admin_role));
    Router::new().route(
        "/commands/{command_id}",
        middleware::require_operator(state, command),
    )
}

async fn require_admin_role(
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(identity): Extension<OperatorIdentity>,
    request: Request,
    next: Next,
) -> Response {
    if let Err(error) = operator::require_admin(identity.role()) {
        return ApiError::from_operator(error, correlation_id).into_response();
    }
    next.run(request).await
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
#[allow(clippy::doc_markdown)]
pub(crate) struct CommandPath {
    /// Canonical lowercase hyphenated UUIDv7 supplied by the Panel
    command_id: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PutCommandRequest {
    device_id: String,
    kind: String,
    payload_version: i32,
    #[schema(value_type = Object)]
    payload: Box<RawValue>,
    reason_code: Option<String>,
    group_correlation_id: Option<String>,
}

#[utoipa::path(
    put,
    path = "/api/v2/commands/{command_id}",
    operation_id = "putCommand",
    summary = "Create or replay a direct Device Command",
    description = crate::openapi::COMMAND_DESCRIPTION,
    params(CommandPath),
    security(("sessionCookie" = [])),
    request_body = PutCommandRequest,
    responses(
        (status = 200, description = "Identical Command request replayed"),
        (status = 201, description = "Command created"),
        (status = 400, description = "Command ID is not canonical UUIDv7"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Device does not exist or is not enrolled"),
        (status = 409, description = "Command request conflicts"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn put_command(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Path(path): Path<CommandPath>,
    request: Result<Json<PutCommandRequest>, JsonRejection>,
) -> Response {
    let command_id = match CommandId::parse(&path.command_id) {
        Ok(command_id) => command_id,
        Err(error) => return ApiError::from_command(error, correlation_id).into_response(),
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return rejection.into_response();
        }
        Err(_) => {
            return ApiError::from_command(CommandError::RequestInvalid, correlation_id)
                .into_response();
        }
    };
    let input = CommandRequestInput {
        device_id: request.device_id,
        kind: request.kind,
        payload_version: request.payload_version,
        payload: request.payload,
        reason_code: request.reason_code,
        group_correlation_id: request.group_correlation_id,
    };
    match command::put_command(&state.database, &command_id, input, correlation_id, &()).await {
        Ok(CommandOutcome::Created) => StatusCode::CREATED.into_response(),
        Ok(CommandOutcome::Replayed) => StatusCode::OK.into_response(),
        Err(error) => ApiError::from_command(error, correlation_id).into_response(),
    }
}

#[cfg(test)]
mod tests;
