mod cookie;
mod error;
pub(crate) mod handler;
mod middleware;

use std::{path::Path, sync::Arc};

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, header::CACHE_CONTROL},
    middleware::{self as axum_middleware, Next},
    response::Response,
    routing::{MethodRouter, any_service},
};
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

use crate::server_state::ServerState;

use self::error::ApiError;

pub(crate) type AppState = Arc<ServerState>;

type Routes<S = AppState> = Vec<(&'static str, MethodRouter<S>)>;

/// Builds the mounted Server HTTP surface over process-wide business state.
pub(crate) fn router(state: AppState, web_root: &Path) -> Router {
    let static_service =
        any_service(ServeDir::new(web_root).fallback(ServeFile::new(web_root.join("index.html"))))
            .layer(SetResponseHeaderLayer::overriding(
                CACHE_CONTROL,
                HeaderValue::from_static("no-cache"),
            ));
    Router::new()
        .nest("/api/v2", api_v2(&state))
        .fallback_service(static_service)
        .with_state(state)
        .layer(axum_middleware::from_fn(private_namespace_no_store))
        .layer(axum_middleware::from_fn(middleware::request_context))
}

// Cover root method rejection too, so early responses for this namespace
// have the same cache policy as its handlers.
async fn private_namespace_no_store(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let private = path == "/api/v2/operator"
        || path.starts_with("/api/v2/operator/")
        || path == "/api/v2/print"
        || path.starts_with("/api/v2/print/");
    let mut response = next.run(request).await;
    if private {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn route_groups(state: &AppState) -> [Routes; 4] {
    let public = [
        handler::health::public_routes(),
        handler::device_control::public_routes(),
        handler::session::public_routes(),
        handler::operator::public_routes(state.clone()),
        handler::contest::public_routes(),
        handler::print::public_routes(state.clone()),
    ]
    .into_iter()
    .flatten()
    .collect();
    let operator = [
        handler::contest::operator_routes(),
        handler::device::operator_routes(),
        handler::enrollment::operator_routes(),
        handler::import::operator_routes(),
        handler::provisioning::operator_routes(),
        handler::target_submission::operator_routes(),
        handler::session::operator_routes(),
        handler::operator::operator_routes(),
        handler::print::operator_routes(),
    ]
    .into_iter()
    .flatten()
    .collect();
    [
        public,
        operator,
        handler::print::source_routes(),
        handler::print::client_routes(),
    ]
}

fn operator_group(state: AppState, routes: Routes) -> Router<AppState> {
    let authentication =
        axum_middleware::from_fn_with_state(state, middleware::authenticate_operator);
    routes
        .into_iter()
        .fold(Router::new(), |router, (path, methods)| {
            router.route(path, methods.route_layer(authentication.clone()))
        })
}

fn source_group(state: AppState, routes: Routes) -> Router<AppState> {
    let authentication =
        axum_middleware::from_fn_with_state(state, middleware::authenticate_source);
    routes
        .into_iter()
        .fold(Router::new(), |router, (path, methods)| {
            router.route(path, methods.route_layer(authentication.clone()))
        })
}

fn client_group(state: AppState, routes: Routes) -> Router<AppState> {
    let authentication =
        axum_middleware::from_fn_with_state(state, middleware::authenticate_client);
    routes
        .into_iter()
        .fold(Router::new(), |router, (path, methods)| {
            router.route(path, methods.route_layer(authentication.clone()))
        })
}

fn api_v2(state: &AppState) -> Router<AppState> {
    let [public, operator, source, client] = route_groups(state);
    public
        .into_iter()
        .fold(Router::new(), |router, (path, methods)| {
            router.route(path, methods)
        })
        .merge(operator_group(state.clone(), operator))
        .merge(source_group(state.clone(), source))
        .merge(client_group(state.clone(), client))
        .fallback(not_found)
        .layer(axum_middleware::from_fn(middleware::reject_head))
        .layer(axum_middleware::from_fn_with_state(
            state.lifecycle().clone(),
            middleware::complete_mutation,
        ))
}

async fn not_found() -> ApiError {
    ApiError::not_found("unmounted_route")
}

#[cfg(test)]
pub(crate) mod tests;
