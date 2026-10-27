use axum::{
    extract::Request,
    http::Method,
    middleware::Next,
    response::{IntoResponse as _, Response},
};
use tracing::{Instrument as _, instrument::WithSubscriber as _};

use super::super::error::ApiError;

fn requires_completion(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

pub(in crate::http) async fn complete_mutation(request: Request, next: Next) -> Response {
    if !requires_completion(request.method()) {
        return next.run(request).await;
    }
    let dispatcher = tracing::dispatcher::get_default(Clone::clone);
    match tokio::spawn(
        next.run(request)
            .instrument(tracing::Span::current())
            .with_subscriber(dispatcher),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => ApiError::internal_error_from(
            crate::task::TaskFailure::from(error),
            "request_task_failed",
        )
        .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scoped_dispatcher_is_preserved_and_panic_payloads_are_redacted() {
        use crate::{
            config::LogLevel,
            logging::tests::{CapturedLogs, SubscriberTestGuard},
        };
        use axum::{
            Router,
            body::{Body, to_bytes},
            http::{Request, StatusCode},
            middleware,
            routing::post,
        };
        use tower::ServiceExt as _;

        async fn panicking_handler() -> StatusCode {
            tracing::info!("request_task_dispatcher_canary");
            panic!("request_task_secret_canary");
        }

        let _guard = SubscriberTestGuard::acquire();
        let captured = CapturedLogs::default();
        let application = Router::new()
            .route("/", post(panicking_handler))
            .layer(middleware::from_fn(complete_mutation));
        let response = application
            .oneshot(
                Request::post("/")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("request: {error}")),
            )
            .with_subscriber(captured.subscriber(LogLevel::Trace))
            .await
            .unwrap_or_else(|error| panic!("router: {error}"));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), 4096)
            .await
            .unwrap_or_else(|error| panic!("body: {error}"));
        let expected = to_bytes(
            ApiError::internal_error("request_task_failed")
                .into_response()
                .into_body(),
            4096,
        )
        .await
        .unwrap_or_else(|error| panic!("body: {error}"));
        assert_eq!(body, expected);
        let events = captured
            .text()
            .unwrap_or_else(|()| panic!("captured events"));
        assert!(events.contains("request_task_dispatcher_canary"));
        assert_eq!(events.matches("cause=\"request_task_failed\"").count(), 1);
        assert!(!events.contains("request_task_secret_canary"));
    }

    #[test]
    fn only_safe_methods_run_in_the_caller() {
        for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(!requires_completion(&method));
        }
        for method in [
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::CONNECT,
            Method::TRACE,
            Method::from_bytes(b"CUSTOM").unwrap_or_else(|error| panic!("method: {error}")),
        ] {
            assert!(requires_completion(&method));
        }
    }
}
