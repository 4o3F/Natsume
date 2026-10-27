use crate::http::Routes;

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
    Extension, Json,
    extract::{FromRequest, Path, Request, State, rejection::JsonRejection},
    http::StatusCode,
    middleware::{self as axum_middleware, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use uuid::Uuid;

pub(in crate::http) fn public_routes(state: AppState) -> Routes {
    let logged_out = axum_middleware::from_fn_with_state(state, require_logged_out);
    vec![
        (
            "/operator/register/inspect",
            post(inspect_registration).route_layer(logged_out.clone()),
        ),
        (
            "/operator/register",
            post(register_operator).route_layer(logged_out.clone()),
        ),
        (
            "/operator/password/reset/inspect",
            post(inspect_password_reset).route_layer(logged_out.clone()),
        ),
        (
            "/operator/password/reset",
            post(reset_password).route_layer(logged_out),
        ),
    ]
}

pub(in crate::http) fn operator_routes() -> Routes {
    vec![
        (
            "/operator/accounts",
            middleware::require_admin(get(list_accounts)),
        ),
        (
            "/operator/accounts/{operator_id}",
            middleware::require_admin(patch(update_account).delete(delete_account)),
        ),
        (
            "/operator/invitations",
            middleware::require_admin(get(list_invitations).post(create_invitation)),
        ),
        (
            "/operator/invitations/{invite_id}",
            middleware::require_admin(delete(revoke_invitation)),
        ),
        (
            "/operator/invitations/{invite_id}/actions/regenerate",
            middleware::require_admin(post(regenerate_invitation)),
        ),
        (
            "/operator/accounts/{operator_id}/password-resets",
            middleware::require_admin(post(create_password_reset)),
        ),
        ("/operator/password/change", post(change_password)),
    ]
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

fn parse_id(value: &str) -> Result<Uuid, ApiError> {
    let id = Uuid::parse_str(value)
        // Malformed request input is a domain rejection; parser detail is irrelevant.
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
        .map_err(|error| ApiError::internal_error_from(error, "operator_clock_invalid"))
}

fn clear_session(response: Response) -> Response {
    cookie::with_clearing_session_cookie(response).unwrap_or_else(|error| {
        ApiError::internal_error_from(&error, "operator_clearing_cookie_failed").into_response()
    })
}

pub(crate) async fn list_accounts(State(state): State<AppState>) -> Response {
    match state.operator().list_accounts().await {
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

pub(crate) async fn update_account(
    State(state): State<AppState>,
    Path(path): Path<OperatorAccountPath>,
    request: Request,
) -> Response {
    let target = match parse_id(&path.operator_id) {
        Ok(target) => target,
        Err(error) => return error.into_response(),
    };
    let Ok(Json(request)) = Json::<OperatorRoleRequest>::from_request(request, &state).await else {
        return ApiError::invalid_request("operator_form_rejected").into_response();
    };
    match state
        .operator()
        .change_role(target, request.role.into())
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

pub(crate) async fn delete_account(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    Path(path): Path<OperatorAccountPath>,
) -> Response {
    let target = match parse_id(&path.operator_id) {
        Ok(target) => target,
        Err(error) => return error.into_response(),
    };
    match state.operator().delete_account(target).await {
        Ok(()) if target == actor.operator_id() => {
            clear_session(StatusCode::NO_CONTENT.into_response())
        }
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

pub(crate) async fn list_invitations(State(state): State<AppState>) -> Response {
    let invitations = match state.operator().list_invitations().await {
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

pub(crate) async fn create_invitation(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    request: Result<Json<OperatorRoleRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return ApiError::invalid_request("operator_form_rejected").into_response();
    };
    match state
        .operator()
        .issue_invitation(actor.operator_id(), request.role.into())
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

pub(crate) async fn revoke_invitation(
    State(state): State<AppState>,
    Path(path): Path<OperatorInvitationPath>,
) -> Response {
    let id = match parse_id(&path.invite_id) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    match state.operator().revoke_invitation(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_operator(error).into_response(),
    }
}

pub(crate) async fn regenerate_invitation(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    Path(path): Path<OperatorInvitationPath>,
) -> Response {
    let id = match parse_id(&path.invite_id) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    match state
        .operator()
        .regenerate_invitation(actor.operator_id(), id)
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

pub(crate) async fn create_password_reset(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    Path(path): Path<OperatorAccountPath>,
) -> Response {
    let target = match parse_id(&path.operator_id) {
        Ok(target) => target,
        Err(error) => return error.into_response(),
    };
    match state
        .operator()
        .issue_password_reset(actor.operator_id(), target)
        .await
    {
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

pub(crate) async fn inspect_registration(
    State(state): State<AppState>,
    request: Result<Json<OperatorTokenRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return ApiError::invalid_request("operator_form_rejected").into_response();
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

pub(crate) async fn register_operator(
    State(state): State<AppState>,
    request: Result<Json<OperatorRegistrationRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return ApiError::invalid_request("operator_form_rejected").into_response();
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

pub(crate) async fn inspect_password_reset(
    State(state): State<AppState>,
    request: Result<Json<OperatorTokenRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return ApiError::invalid_request("operator_form_rejected").into_response();
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

pub(crate) async fn reset_password(
    State(state): State<AppState>,
    request: Result<Json<OperatorPasswordResetRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return ApiError::invalid_request("operator_form_rejected").into_response();
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

pub(crate) async fn change_password(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    request: Result<Json<OperatorPasswordChangeRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return ApiError::invalid_request("operator_form_rejected").into_response();
    };
    match state
        .operator()
        .change_password(
            actor.operator_id(),
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
