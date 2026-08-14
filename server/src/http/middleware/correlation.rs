use std::time::Instant;

use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};
use uuid::Uuid;

use crate::audit::CorrelationId;

use super::CORRELATION_ID_HEADER;

pub(in crate::http) async fn correlation_id(mut request: Request, next: Next) -> Response {
    let correlation_id = CorrelationId::from_uuid(Uuid::now_v7());
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started_at = Instant::now();
    request.extensions_mut().insert(correlation_id);
    let mut response = next.run(request).await;
    let status = response.status();
    if let Ok(value) = HeaderValue::from_str(&correlation_id.as_text()) {
        response.headers_mut().insert(CORRELATION_ID_HEADER, value);
    }
    let duration_us = u64::try_from(started_at.elapsed().as_micros()).unwrap_or(u64::MAX);
    tracing::info!(
        method = %method,
        path = %path,
        status = status.as_u16(),
        correlation_id = %correlation_id.as_text(),
        duration_us,
        "HTTP request completed"
    );
    response
}
