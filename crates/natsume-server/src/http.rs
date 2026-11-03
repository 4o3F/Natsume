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

use crate::{
    component::{operator::OperatorComponent, print::PrintComponent},
    journal::Journal,
    lifecycle::ServerLifecycle,
    server_state::ServerState,
};

use self::error::ApiError;

pub(crate) type AppState = Arc<ServerState>;

pub(crate) type Routes<S = AppState> = Vec<(&'static str, MethodRouter<S>)>;

/// Binds a feature's narrow state to each of its routes.
pub(crate) fn bind_routes<S>(routes: Routes<S>, state: &S) -> Routes<()>
where
    S: Clone + Send + Sync + 'static,
{
    routes
        .into_iter()
        .map(|(path, methods)| (path, methods.with_state(state.clone())))
        .collect()
}

pub(crate) struct AccessRoutes {
    /// Mounted without principal authentication.
    pub(crate) public: Routes<()>,
    /// Mounted with operator session authentication.
    pub(crate) operator: Routes<()>,
    /// Mounted with print source bearer authentication.
    pub(crate) source: Routes<()>,
    /// Mounted with print client bearer authentication.
    pub(crate) client: Routes<()>,
}

impl AccessRoutes {
    pub(crate) fn new() -> Self {
        Self {
            public: Vec::new(),
            operator: Vec::new(),
            source: Vec::new(),
            client: Vec::new(),
        }
    }

    pub(crate) fn merge(mut self, other: Self) -> Self {
        self.public.extend(other.public);
        self.operator.extend(other.operator);
        self.source.extend(other.source);
        self.client.extend(other.client);
        self
    }
}

/// Assembles legacy routes during migration; removed in Step 11.
pub(crate) fn router(state: AppState, routes: AccessRoutes, web_root: &Path) -> Router {
    let routes = legacy_routes().merge(routes);
    let application = mount(
        routes,
        state.operator().clone(),
        state.print().clone(),
        state.journal().clone(),
        state.lifecycle().clone(),
        web_root,
    );
    drop(state);
    application
}

pub(crate) fn mount(
    routes: AccessRoutes,
    operator: Arc<OperatorComponent>,
    print: Arc<PrintComponent>,
    journal: Journal,
    lifecycle: ServerLifecycle,
    web_root: &Path,
) -> Router {
    let api_v2 = routes
        .public
        .into_iter()
        .fold(Router::new(), |router, (path, methods)| {
            router.route(path, methods)
        })
        .merge(operator_group(operator, routes.operator))
        .merge(source_group(print.clone(), routes.source))
        .merge(client_group(print, routes.client))
        .fallback(not_found)
        .layer(axum_middleware::from_fn(middleware::reject_head))
        .layer(axum_middleware::from_fn_with_state(
            journal.clone(),
            middleware::audit::audit_request,
        ))
        .layer(axum_middleware::from_fn_with_state(
            middleware::CompletionState::new(lifecycle, journal),
            middleware::complete_mutation,
        ));
    let static_service =
        any_service(ServeDir::new(web_root).fallback(ServeFile::new(web_root.join("index.html"))))
            .layer(SetResponseHeaderLayer::overriding(
                CACHE_CONTROL,
                HeaderValue::from_static("no-cache"),
            ));
    Router::new()
        .nest("/api/v2", api_v2)
        .fallback_service(static_service)
        .layer(axum_middleware::from_fn(private_namespace_no_store))
        .layer(axum_middleware::from_fn(middleware::request_context))
}

// Cover root method rejection too, so early responses for this namespace
// have the same cache policy as its handlers.
async fn private_namespace_no_store(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let private = path == "/api/v2/journal"
        || path.starts_with("/api/v2/journal/")
        || path == "/api/v2/operator"
        || path.starts_with("/api/v2/operator/")
        || path == "/api/v2/print"
        || path.starts_with("/api/v2/print/")
        || path == "/api/v2/photo"
        || path.starts_with("/api/v2/photo/");
    let mut response = next.run(request).await;
    if private {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn legacy_routes() -> AccessRoutes {
    AccessRoutes {
        public: handler::health::public_routes(),
        ..AccessRoutes::new()
    }
}

fn operator_group(state: Arc<OperatorComponent>, routes: Routes<()>) -> Router {
    let authentication =
        axum_middleware::from_fn_with_state(state, middleware::authenticate_operator);
    routes
        .into_iter()
        .fold(Router::new(), |router, (path, methods)| {
            router.route(path, methods.route_layer(authentication.clone()))
        })
}

fn source_group(state: Arc<PrintComponent>, routes: Routes<()>) -> Router {
    let authentication =
        axum_middleware::from_fn_with_state(state, middleware::authenticate_source);
    routes
        .into_iter()
        .fold(Router::new(), |router, (path, methods)| {
            router.route(path, methods.route_layer(authentication.clone()))
        })
}

fn client_group(state: Arc<PrintComponent>, routes: Routes<()>) -> Router {
    let authentication =
        axum_middleware::from_fn_with_state(state, middleware::authenticate_client);
    routes
        .into_iter()
        .fold(Router::new(), |router, (path, methods)| {
            router.route(path, methods.route_layer(authentication.clone()))
        })
}

async fn not_found() -> ApiError {
    ApiError::not_found("unmounted_route")
}

#[cfg(test)]
pub(crate) mod tests;
