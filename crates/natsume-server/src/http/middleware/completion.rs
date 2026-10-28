use axum::{
    extract::{Request, State},
    http::Method,
    middleware::Next,
    response::{IntoResponse as _, Response},
};
use tracing::{Instrument as _, instrument::WithSubscriber as _};

use super::super::error::ApiError;

fn requires_completion(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

pub(in crate::http) async fn complete_mutation(
    State(lifecycle): State<crate::lifecycle::ServerLifecycle>,
    mut request: Request,
    next: Next,
) -> Response {
    if !requires_completion(request.method()) {
        return next.run(request).await;
    }
    let Some(admission) = lifecycle.admit() else {
        return ApiError::unavailable("server_stopping").into_response();
    };
    request.extensions_mut().insert(admission.clone());
    let dispatcher = tracing::dispatcher::get_default(Clone::clone);
    let task = admission.spawn(
        next.run(request)
            .instrument(tracing::Span::current())
            .with_subscriber(dispatcher),
    );
    match task.await {
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
        let application = Router::new().route("/", post(panicking_handler)).layer(
            middleware::from_fn_with_state(
                crate::lifecycle::ServerLifecycle::new(),
                complete_mutation,
            ),
        );
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

    #[tokio::test]
    async fn cancelled_mutation_finishes_before_the_journal_closes()
    -> Result<(), crate::test_error::TestError> {
        use crate::journal::{
            Journal, JournalComponent, JournalContext,
            event::{Event, payload::ReviewCreated},
        };
        use crate::journal_schema::journal_events;
        use axum::{
            Router,
            body::Body,
            extract::State,
            http::{Request, StatusCode},
            middleware,
            routing::post,
        };
        use diesel::{Connection as _, QueryDsl as _, RunQueryDsl as _};
        use std::sync::Arc;
        use tokio::sync::Notify;
        use tower::ServiceExt as _;
        type HandlerState = (Journal, Arc<Notify>, Arc<Notify>);
        async fn handler(
            State((journal, entered, release)): State<HandlerState>,
            axum::Extension(admission): axum::Extension<crate::lifecycle::Admission>,
        ) -> StatusCode {
            entered.notify_one();
            release.notified().await;
            admission
                .spawn(async move {
                    journal.emit(
                        Event::ReviewCreated(ReviewCreated {}),
                        JournalContext::default(),
                    );
                })
                .await
                .unwrap_or_else(|error| panic!("child mutation: {error}"));
            StatusCode::NO_CONTENT
        }
        let root = tempfile::tempdir()?;
        let path = root.path().join("journal.db");
        let component = JournalComponent::start(
            path.clone(),
            crate::config::JournalConfig {
                min_free_bytes: 1,
                ..crate::config::JournalConfig::default()
            },
        );
        crate::journal::tests::ready(&component).await?;
        let journal = component.emitter();
        let lifecycle = crate::lifecycle::ServerLifecycle::new();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let application = Router::new()
            .route("/", post(handler))
            .with_state((journal.clone(), Arc::clone(&entered), Arc::clone(&release)))
            .layer(middleware::from_fn_with_state(
                lifecycle.clone(),
                complete_mutation,
            ));
        let caller = tokio::spawn(
            application
                .clone()
                .oneshot(Request::post("/").body(Body::empty())?),
        );
        entered.notified().await;
        caller.abort();
        lifecycle.begin_shutdown();
        let waiting = lifecycle.wait();
        tokio::pin!(waiting);
        assert!(futures_util::poll!(&mut waiting).is_pending());
        let rejected = application
            .oneshot(Request::post("/").body(Body::empty())?)
            .await?;
        assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
        release.notify_one();
        waiting.await;
        component.shutdown().await;
        assert_eq!(component.stats().categories.device.dropped, 0);
        let mut connection = diesel::sqlite::SqliteConnection::establish(
            path.to_str()
                .ok_or(crate::test_error::TestError::missing("path"))?,
        )?;
        let kinds = journal_events::table
            .select(journal_events::kind)
            .order(journal_events::id)
            .load::<String>(&mut connection)?;
        assert_eq!(
            kinds,
            ["server_started", "review_created", "server_stopping"]
        );
        Ok(())
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
