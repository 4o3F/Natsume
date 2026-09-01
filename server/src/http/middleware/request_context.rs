use std::time::Instant;

use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};
use opentelemetry::propagation::{Extractor, TextMapPropagator as _};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing::Instrument as _;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

struct HeaderExtractor<'headers>(&'headers axum::http::HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(axum::http::HeaderName::as_str).collect()
    }
}

pub(in crate::http) async fn request_context(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("<unmatched>", MatchedPath::as_str)
        .to_owned();
    let started_at = Instant::now();
    let span_name = format!("{} {route}", method.as_str());

    let request_span = tracing::info_span!(
        "http_request",
        "otel.name" = span_name.as_str(),
        "otel.kind" = "server",
        "otel.status_code" = tracing::field::Empty,
        "http.request.method" = method.as_str(),
        "http.route" = route.as_str(),
        "http.response.status_code" = tracing::field::Empty,
        "request.outcome" = tracing::field::Empty,
        actor_kind = tracing::field::Empty,
        actor_id = tracing::field::Empty,
    );
    let parent_context = TraceContextPropagator::new().extract(&HeaderExtractor(request.headers()));
    let _parent_result = request_span.set_parent(parent_context);

    async move {
        let response = next.run(request).await;
        let status = response.status();
        let duration_us = u64::try_from(started_at.elapsed().as_micros()).unwrap_or(u64::MAX);
        let outcome = if status.is_server_error() {
            "server_error"
        } else if status.is_client_error() {
            "client_error"
        } else {
            "success"
        };
        let span = tracing::Span::current();
        span.record("http.response.status_code", u64::from(status.as_u16()));
        span.record("request.outcome", outcome);
        if status.is_server_error() {
            span.record("otel.status_code", "ERROR");
        }
        tracing::info!(
            method = %method,
            route,
            status = status.as_u16(),
            duration_us,
            "HTTP request completed"
        );
        response
    }
    .instrument(request_span)
    .await
}
