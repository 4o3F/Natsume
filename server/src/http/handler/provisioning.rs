use axum::{
    Extension, Json, Router,
    extract::{Request, State},
    middleware as axum_middleware,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::component::{operator::OperatorIdentity, provisioning::ProvisioningWindow};

use super::super::{AppState, error::ApiError, middleware};

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    let open =
        post(open_provisioning_window).route_layer(axum_middleware::from_fn(require_admin_role));
    let close =
        post(close_provisioning_window).route_layer(axum_middleware::from_fn(require_admin_role));
    Router::new()
        .route(
            "/provisioning-window",
            middleware::operator_get(state.clone(), get_provisioning_window),
        )
        .route(
            "/provisioning-window/actions/open",
            middleware::require_operator(state.clone(), open),
        )
        .route(
            "/provisioning-window/actions/close",
            middleware::require_operator(state, close),
        )
}

async fn require_admin_role(
    Extension(identity): Extension<OperatorIdentity>,
    request: Request,
    next: Next,
) -> Response {
    if let Err(error) = identity.require_admin() {
        return ApiError::from_operator(error).into_response();
    }
    next.run(request).await
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvisioningWindowResponse {
    #[schema(inline)]
    state: ProvisioningWindowResponseState,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum ProvisioningWindowResponseState {
    Closed,
    Open,
}

impl From<ProvisioningWindow> for ProvisioningWindowResponse {
    fn from(window: ProvisioningWindow) -> Self {
        Self {
            state: if window.is_open() {
                ProvisioningWindowResponseState::Open
            } else {
                ProvisioningWindowResponseState::Closed
            },
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/provisioning-window",
    operation_id = "getProvisioningWindow",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current provisioning-window fact", body = ProvisioningWindowResponse),
        (status = 401, description = "Session authentication failed")
    )
)]
pub(crate) async fn get_provisioning_window(State(state): State<AppState>) -> Response {
    let window = state.provisioning().read_window().await;
    Json(ProvisioningWindowResponse::from(window)).into_response()
}

#[utoipa::path(
    post,
    path = "/api/v2/provisioning-window/actions/open",
    operation_id = "openProvisioningWindow",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Provisioning window opened or already open", body = ProvisioningWindowResponse),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required")
    )
)]
pub(crate) async fn open_provisioning_window(State(state): State<AppState>) -> Response {
    let window = state.provisioning().open_window().await;
    Json(ProvisioningWindowResponse::from(window)).into_response()
}

#[utoipa::path(
    post,
    path = "/api/v2/provisioning-window/actions/close",
    operation_id = "closeProvisioningWindow",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Provisioning window closed or already closed", body = ProvisioningWindowResponse),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required")
    )
)]
pub(crate) async fn close_provisioning_window(State(state): State<AppState>) -> Response {
    let window = state.provisioning().close_window().await;
    Json(ProvisioningWindowResponse::from(window)).into_response()
}
