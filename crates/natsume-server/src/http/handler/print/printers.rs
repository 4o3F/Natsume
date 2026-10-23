use axum::{
    Json, Router,
    extract::{ConnectInfo, Request, State, rejection::JsonRejection},
    http::{StatusCode, header},
    middleware::{self as axum_middleware, Next},
    response::{IntoResponse as _, Response},
    routing::{delete, post},
};
use serde::Deserialize;

use super::{ApiError, AppState, PrintError, PrintPathId, middleware};
use crate::tls::PeerAddress;

#[derive(Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintPrinterCreate {
    name: String,
}

#[derive(Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintClientPair {
    code: String,
}

pub(super) fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/print/printers",
            middleware::require_admin(state.clone(), post(create).get(list)),
        )
        .route(
            "/print/printers/{printer_id}/pairing-code",
            middleware::require_admin(state.clone(), post(issue_code)),
        )
        .route(
            "/print/printers/{printer_id}/binding",
            middleware::require_admin(state.clone(), delete(clear_binding)),
        )
        .route(
            "/print/client/pair",
            post(pair).route_layer(axum_middleware::from_fn_with_state(state, pairing_limit)),
        )
        .route("/print/client/confirmation", post(confirm))
}

async fn pairing_limit(State(state): State<AppState>, request: Request, next: Next) -> Response {
    // Only the TLS listener's socket-observed IP is trusted, never forwarded headers.
    let Some(ConnectInfo(PeerAddress(peer))) =
        request.extensions().get::<ConnectInfo<PeerAddress>>()
    else {
        return ApiError::unavailable("print_pairing_peer_unavailable").into_response();
    };
    match state.print().admit_pairing(*peer) {
        Ok(()) => next.run(request).await,
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn create(
    State(state): State<AppState>,
    body: Result<Json<PrintPrinterCreate>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return ApiError::invalid_request("print_printer_json").into_response();
    };
    match state.print().create_printer(body.name).await {
        Ok(printer) => Json(printer).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn list(State(state): State<AppState>) -> Response {
    match state.print().list_printers().await {
        Ok(printers) => Json(printers).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn issue_code(State(state): State<AppState>, PrintPathId(id): PrintPathId) -> Response {
    match state.print().issue_pairing_code(id).await {
        Ok(code) => Json(code).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn clear_binding(State(state): State<AppState>, PrintPathId(id): PrintPathId) -> Response {
    match state.print().clear_printer_binding(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn pair(
    State(state): State<AppState>,
    body: Result<Json<PrintClientPair>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return ApiError::invalid_request("print_pairing_json").into_response();
    };
    match state.print().pair_client(&body.code).await {
        Ok(client) => Json(client).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

async fn confirm(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    let mut authorization = headers.get_all(header::AUTHORIZATION).iter();
    let token = authorization
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let Some(token) = token.filter(|_| authorization.next().is_none()) else {
        return ApiError::from_print(PrintError::Authentication).into_response();
    };
    match state.print().confirm_client(token).await {
        Ok(printer) => Json(printer).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}
