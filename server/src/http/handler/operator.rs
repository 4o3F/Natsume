mod dto;

use self::dto::SecretValue;
pub(crate) use self::dto::{
    OperatorAccountPath, OperatorAccountResponse, OperatorInvitationIssuedResponse,
    OperatorInvitationPath, OperatorInvitationResponse, OperatorPasswordChangeRequest,
    OperatorPasswordResetInspectionResponse, OperatorPasswordResetIssuedResponse,
    OperatorPasswordResetRequest, OperatorRegistrationInspectionResponse,
    OperatorRegistrationRequest, OperatorRoleRequest, OperatorTokenRequest,
};
use super::super::{AppState, cookie, error::ApiError, middleware};
use crate::component::operator::{OperatorError, OperatorIdentity};
use axum::{
    Extension, Json, Router,
    extract::{FromRequest, Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self as axum_middleware, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use serde::de::DeserializeOwned;
use std::time::Duration;
use tower_http::{limit::RequestBodyLimitLayer, set_header::SetResponseHeaderLayer};
use uuid::Uuid;

const FORM_BODY_LIMIT: usize = 24 * 1024;
const FORM_BODY_TIMEOUT: Duration = Duration::from_secs(5);

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    let public = Router::new()
        .route("/operator/register/inspect", post(inspect_registration))
        .route("/operator/register", post(register_operator))
        .route(
            "/operator/password/reset/inspect",
            post(inspect_password_reset),
        )
        .route("/operator/password/reset", post(reset_password))
        .route_layer(axum_middleware::from_fn_with_state(
            state.clone(),
            require_logged_out,
        ));
    Router::new()
        .route(
            "/operator/accounts",
            middleware::require_admin(state.clone(), get(list_accounts)),
        )
        .route(
            "/operator/accounts/{operator_id}",
            middleware::require_admin(state.clone(), patch(update_account).delete(delete_account)),
        )
        .route(
            "/operator/invitations",
            middleware::require_admin(state.clone(), get(list_invitations).post(create_invitation)),
        )
        .route(
            "/operator/invitations/{invite_id}",
            middleware::require_admin(state.clone(), delete(revoke_invitation)),
        )
        .route(
            "/operator/invitations/{invite_id}/actions/regenerate",
            middleware::require_admin(state.clone(), post(regenerate_invitation)),
        )
        .route(
            "/operator/accounts/{operator_id}/password-resets",
            middleware::require_admin(state.clone(), post(create_password_reset)),
        )
        .route(
            "/operator/password/change",
            middleware::require_operator(state, post(change_password)),
        )
        .merge(public)
        .layer(RequestBodyLimitLayer::new(FORM_BODY_LIMIT))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
}

async fn require_logged_out(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if let Ok(credential) = cookie::session_credential(request.headers()) {
        match state.operator().authenticate_session(credential).await {
            Ok(_) => return ApiError::operator_logout_required().into_response(),
            Err(OperatorError::SessionAuthenticationFailed) => {}
            Err(error) => return ApiError::from_operator(error).into_response(),
        }
    }
    next.run(request).await
}

async fn read_json<T: DeserializeOwned>(request: Request, state: &AppState) -> Result<T, Response> {
    match tokio::time::timeout(FORM_BODY_TIMEOUT, Json::<T>::from_request(request, state)).await {
        Ok(Ok(Json(value))) => Ok(value),
        Ok(Err(error)) if error.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            Err(StatusCode::PAYLOAD_TOO_LARGE.into_response())
        }
        Ok(Err(_)) => Err(ApiError::invalid_request("operator_form_rejected").into_response()),
        Err(_) => Err(StatusCode::REQUEST_TIMEOUT.into_response()),
    }
}

fn parse_id(value: &str) -> Result<Uuid, ApiError> {
    let id = Uuid::parse_str(value)
        .map_err(|_| ApiError::invalid_request("operator_id_not_canonical_uuid_v7"))?;
    if id.get_version_num() != 7 || id.to_string() != value {
        return Err(ApiError::invalid_request(
            "operator_id_not_canonical_uuid_v7",
        ));
    }
    Ok(id)
}

fn now_ms() -> Result<i64, ApiError> {
    i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| ApiError::internal_error("operator_clock_invalid"))
}

fn clear_session(response: Response) -> Response {
    cookie::with_clearing_session_cookie(response).unwrap_or_else(|()| {
        ApiError::internal_error("operator_clearing_cookie_failed").into_response()
    })
}

#[utoipa::path(
    get,
    path = "/api/v2/operator/accounts",
    operation_id = "listOperatorAccounts",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current response", body = [OperatorAccountResponse]),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 500, description = "Internal failure"),
    )
)]
pub(crate) async fn list_accounts(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
) -> Response {
    match state.operator().list_accounts(actor).await {
        Ok(accounts) => Json(
            accounts
                .into_iter()
                .map(OperatorAccountResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v2/operator/accounts/{operator_id}",
    operation_id = "updateOperatorAccount",
    params(OperatorAccountPath),
    security(("sessionCookie" = [])),
    request_body = OperatorRoleRequest,
    responses(
        (status = 204, description = "Completed"),
        (status = 400, description = "Invalid closed form, identifier, password policy, confirmation or current password"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Operator not found"),
        (status = 408, description = "Form body was not received within 5 seconds"),
        (status = 409, description = "Login name conflict, last administrator, changed credentials or logout required; see code"),
        (status = 413, description = "Request body exceeds 24 KiB"),
        (status = 500, description = "Internal failure"),
    )
)]
pub(crate) async fn update_account(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    Path(path): Path<OperatorAccountPath>,
    request: Request,
) -> Response {
    let target = match parse_id(&path.operator_id) {
        Ok(target) => target,
        Err(error) => return error.into_response(),
    };
    let request: OperatorRoleRequest = match read_json(request, &state).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    match state
        .operator()
        .change_role(actor, target, request.role.into())
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v2/operator/accounts/{operator_id}",
    operation_id = "deleteOperatorAccount",
    params(OperatorAccountPath),
    security(("sessionCookie" = [])),
    responses(
        (status = 204, description = "Completed"),
        (status = 400, description = "Invalid closed form, identifier, password policy, confirmation or current password"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Operator not found"),
        (status = 409, description = "Login name conflict, last administrator, changed credentials or logout required; see code"),
        (status = 500, description = "Internal failure"),
    )
)]
pub(crate) async fn delete_account(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    Path(path): Path<OperatorAccountPath>,
) -> Response {
    let target = match parse_id(&path.operator_id) {
        Ok(target) => target,
        Err(error) => return error.into_response(),
    };
    match state.operator().delete_account(actor, target).await {
        Ok(()) if target == actor.operator_id() => {
            clear_session(StatusCode::NO_CONTENT.into_response())
        }
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/operator/invitations",
    operation_id = "listOperatorInvitations",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current response", body = [OperatorInvitationResponse]),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 500, description = "Internal failure"),
    )
)]
pub(crate) async fn list_invitations(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
) -> Response {
    let invitations = match state.operator().list_invitations(actor).await {
        Ok(invitations) => invitations,
        Err(error) => return ApiError::from_operator(error).into_response(),
    };
    let now = match now_ms() {
        Ok(now) => now,
        Err(error) => return error.into_response(),
    };
    Json(
        invitations
            .into_iter()
            .map(|invitation| OperatorInvitationResponse::new(&invitation, now))
            .collect::<Vec<_>>(),
    )
    .into_response()
}

#[utoipa::path(
    post,
    path = "/api/v2/operator/invitations",
    operation_id = "createOperatorInvitation",
    security(("sessionCookie" = [])),
    request_body = OperatorRoleRequest,
    responses(
        (status = 201, description = "Created; secret is returned only once, without establishing a session", body = OperatorInvitationIssuedResponse),
        (status = 400, description = "Invalid closed form, identifier, password policy, confirmation or current password"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 408, description = "Form body was not received within 5 seconds"),
        (status = 413, description = "Request body exceeds 24 KiB"),
        (status = 500, description = "Internal failure"),
    )
)]
pub(crate) async fn create_invitation(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    request: Request,
) -> Response {
    let request: OperatorRoleRequest = match read_json(request, &state).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    match state
        .operator()
        .issue_invitation(actor, request.role.into())
        .await
    {
        Ok(issued) => {
            let now = match now_ms() {
                Ok(now) => now,
                Err(error) => return error.into_response(),
            };
            (
                StatusCode::CREATED,
                Json(OperatorInvitationIssuedResponse {
                    invitation: OperatorInvitationResponse::new(&issued.invitation, now),
                    token: SecretValue::new(issued.token.expose()),
                }),
            )
                .into_response()
        }
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v2/operator/invitations/{invite_id}",
    operation_id = "revokeOperatorInvitation",
    params(OperatorInvitationPath),
    security(("sessionCookie" = [])),
    responses(
        (status = 204, description = "Completed"),
        (status = 400, description = "Invalid closed form, identifier, password policy, confirmation or current password"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 500, description = "Internal failure"),
    )
)]
pub(crate) async fn revoke_invitation(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    Path(path): Path<OperatorInvitationPath>,
) -> Response {
    let id = match parse_id(&path.invite_id) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    match state.operator().revoke_invitation(actor, id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/operator/invitations/{invite_id}/actions/regenerate",
    operation_id = "regenerateOperatorInvitation",
    params(OperatorInvitationPath),
    security(("sessionCookie" = [])),
    responses(
        (status = 201, description = "Created; secret is returned only once, without establishing a session", body = OperatorInvitationIssuedResponse),
        (status = 400, description = "Invalid closed form, identifier, password policy, confirmation or current password"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 410, description = "Link is unavailable, expired, replaced, revoked or already consumed"),
        (status = 500, description = "Internal failure"),
    )
)]
pub(crate) async fn regenerate_invitation(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    Path(path): Path<OperatorInvitationPath>,
) -> Response {
    let id = match parse_id(&path.invite_id) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    match state.operator().regenerate_invitation(actor, id).await {
        Ok(issued) => {
            let now = match now_ms() {
                Ok(now) => now,
                Err(error) => return error.into_response(),
            };
            (
                StatusCode::CREATED,
                Json(OperatorInvitationIssuedResponse {
                    invitation: OperatorInvitationResponse::new(&issued.invitation, now),
                    token: SecretValue::new(issued.token.expose()),
                }),
            )
                .into_response()
        }
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/operator/accounts/{operator_id}/password-resets",
    operation_id = "createOperatorPasswordReset",
    params(OperatorAccountPath),
    security(("sessionCookie" = [])),
    responses(
        (status = 201, description = "Created; secret is returned only once, without establishing a session", body = OperatorPasswordResetIssuedResponse),
        (status = 400, description = "Invalid closed form, identifier, password policy, confirmation or current password"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Operator not found"),
        (status = 500, description = "Internal failure"),
    )
)]
pub(crate) async fn create_password_reset(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    Path(path): Path<OperatorAccountPath>,
) -> Response {
    let target = match parse_id(&path.operator_id) {
        Ok(target) => target,
        Err(error) => return error.into_response(),
    };
    match state.operator().issue_password_reset(actor, target).await {
        Ok(issued) => (
            StatusCode::CREATED,
            Json(OperatorPasswordResetIssuedResponse {
                operator_id: issued.reset.operator_id,
                reset_id: issued.reset.reset_id,
                username: issued.reset.username,
                expires_at_unix_ms: issued.reset.expires_at_unix_ms,
                token: SecretValue::new(issued.token.expose()),
            }),
        )
            .into_response(),
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/operator/register/inspect",
    operation_id = "inspectOperatorRegistration",
    request_body = OperatorTokenRequest,
    responses(
        (status = 200, description = "Current response", body = OperatorRegistrationInspectionResponse),
        (status = 400, description = "Invalid closed form, identifier, password policy, confirmation or current password"),
        (status = 408, description = "Form body was not received within 5 seconds"),
        (status = 409, description = "Login name conflict, last administrator, changed credentials or logout required; see code"),
        (status = 410, description = "Link is unavailable, expired, replaced, revoked or already consumed"),
        (status = 413, description = "Request body exceeds 24 KiB"),
        (status = 500, description = "Internal failure"),
    )
)]
pub(crate) async fn inspect_registration(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let request: OperatorTokenRequest = match read_json(request, &state).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    match state
        .operator()
        .inspect_invitation(request.token.into_string())
        .await
    {
        Ok(invitation) => Json(OperatorRegistrationInspectionResponse {
            role: invitation.role.into(),
        })
        .into_response(),
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/operator/register",
    operation_id = "registerOperator",
    request_body = OperatorRegistrationRequest,
    responses(
        (status = 201, description = "Created; secret is returned only once, without establishing a session", body = OperatorAccountResponse),
        (status = 400, description = "Invalid closed form, identifier, password policy, confirmation or current password"),
        (status = 408, description = "Form body was not received within 5 seconds"),
        (status = 409, description = "Login name conflict, last administrator, changed credentials or logout required; see code"),
        (status = 410, description = "Link is unavailable, expired, replaced, revoked or already consumed"),
        (status = 413, description = "Request body exceeds 24 KiB"),
        (status = 500, description = "Internal failure"),
        (status = 503, description = "Password work capacity exhausted; retry after 1 second", headers(("Retry-After" = u32, description = "Seconds before retrying; always 1"))),
    )
)]
pub(crate) async fn register_operator(State(state): State<AppState>, request: Request) -> Response {
    let request: OperatorRegistrationRequest = match read_json(request, &state).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    match state
        .operator()
        .register(
            request.token.into_string(),
            request.username.clone(),
            request.password.into_string(),
            request.password_confirmation.into_string(),
        )
        .await
    {
        Ok(identity) => (
            StatusCode::CREATED,
            Json(OperatorAccountResponse {
                operator_id: identity.operator_id(),
                username: request.username,
                role: identity.role().into(),
            }),
        )
            .into_response(),
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/operator/password/reset/inspect",
    operation_id = "inspectOperatorPasswordReset",
    request_body = OperatorTokenRequest,
    responses(
        (status = 200, description = "Current response", body = OperatorPasswordResetInspectionResponse),
        (status = 400, description = "Invalid closed form, identifier, password policy, confirmation or current password"),
        (status = 408, description = "Form body was not received within 5 seconds"),
        (status = 409, description = "Login name conflict, last administrator, changed credentials or logout required; see code"),
        (status = 410, description = "Link is unavailable, expired, replaced, revoked or already consumed"),
        (status = 413, description = "Request body exceeds 24 KiB"),
        (status = 500, description = "Internal failure"),
    )
)]
pub(crate) async fn inspect_password_reset(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let request: OperatorTokenRequest = match read_json(request, &state).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    match state
        .operator()
        .inspect_password_reset(request.token.into_string())
        .await
    {
        Ok(reset) => Json(OperatorPasswordResetInspectionResponse {
            username: reset.username,
        })
        .into_response(),
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/operator/password/reset",
    operation_id = "resetOperatorPassword",
    request_body = OperatorPasswordResetRequest,
    responses(
        (status = 204, description = "Completed"),
        (status = 400, description = "Invalid closed form, identifier, password policy, confirmation or current password"),
        (status = 408, description = "Form body was not received within 5 seconds"),
        (status = 409, description = "Login name conflict, last administrator, changed credentials or logout required; see code"),
        (status = 410, description = "Link is unavailable, expired, replaced, revoked or already consumed"),
        (status = 413, description = "Request body exceeds 24 KiB"),
        (status = 500, description = "Internal failure"),
        (status = 503, description = "Password work capacity exhausted; retry after 1 second", headers(("Retry-After" = u32, description = "Seconds before retrying; always 1"))),
    )
)]
pub(crate) async fn reset_password(State(state): State<AppState>, request: Request) -> Response {
    let request: OperatorPasswordResetRequest = match read_json(request, &state).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    match state
        .operator()
        .consume_password_reset(
            request.token.into_string(),
            request.password.into_string(),
            request.password_confirmation.into_string(),
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/operator/password/change",
    operation_id = "changeOperatorPassword",
    security(("sessionCookie" = [])),
    request_body = OperatorPasswordChangeRequest,
    responses(
        (status = 204, description = "Completed"),
        (status = 400, description = "Invalid closed form, identifier, password policy, confirmation or current password"),
        (status = 401, description = "Session authentication failed"),
        (status = 408, description = "Form body was not received within 5 seconds"),
        (status = 409, description = "Login name conflict, last administrator, changed credentials or logout required; see code"),
        (status = 413, description = "Request body exceeds 24 KiB"),
        (status = 500, description = "Internal failure"),
        (status = 503, description = "Password work capacity exhausted; retry after 1 second", headers(("Retry-After" = u32, description = "Seconds before retrying; always 1"))),
    )
)]
pub(crate) async fn change_password(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    request: Request,
) -> Response {
    let request: OperatorPasswordChangeRequest = match read_json(request, &state).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    match state
        .operator()
        .change_password(
            actor,
            request.current_password.into_string(),
            request.password.into_string(),
            request.password_confirmation.into_string(),
        )
        .await
    {
        Ok(()) => clear_session(StatusCode::NO_CONTENT.into_response()),
        Err(error) => ApiError::from_operator_password_change(error).into_response(),
    }
}

#[cfg(test)]
mod tests;
