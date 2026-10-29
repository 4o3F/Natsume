//! Operator journal HTTP adapters; query semantics belong to `JournalQuery`.
use crate::{
    component::{
        device::DeviceId,
        operator::{OperatorIdentity, OperatorRole},
    },
    http::{AppState, Routes, error::ApiError, middleware},
    journal::read::ReadRequest,
    journal_query::{QueryError, Visibility},
};
use axum::{
    Extension, Json,
    body::Body,
    extract::{Path, RawQuery, State},
    http::{StatusCode, header},
    response::{IntoResponse as _, Response},
    routing::get,
};
use tokio::io::AsyncWriteExt as _;
use tokio_util::codec::{BytesCodec, FramedRead};
mod query;

pub(in crate::http) fn operator_routes() -> Routes {
    vec![
        ("/journal/events", get(events)),
        ("/journal/devices/{device_id}/timeline", get(timeline)),
        ("/journal/device-summaries", get(summaries)),
        ("/journal/export", middleware::require_admin(get(export))),
        ("/journal/status", middleware::require_admin(get(status))),
    ]
}
fn response(result: Result<impl serde::Serialize, QueryError>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => failure(error),
    }
}
fn failure(error: QueryError) -> Response {
    match error {
        QueryError::Forbidden => {
            ApiError::authorization_denied("journal_filter_forbidden").into_response()
        }
        QueryError::Invalid | QueryError::Number { .. } | QueryError::Id { .. } => {
            ApiError::invalid_request("journal_query_invalid").into_response()
        }
        QueryError::Read {
            source: crate::journal::read::ReadError::Unavailable,
        } => ApiError::unavailable("journal_unavailable").into_response(),
        error => ApiError::internal_error_from(error, "journal_query_failed").into_response(),
    }
}
async fn events(
    State(state): State<AppState>,
    Extension(identity): Extension<OperatorIdentity>,
    RawQuery(raw): RawQuery,
) -> Response {
    let request = match query::events(raw.as_deref()) {
        Ok(request) => request,
        Err(error) => return failure(error),
    };
    let visibility = match identity.role() {
        OperatorRole::Admin => Visibility::All,
        OperatorRole::Viewer => Visibility::DeviceOnly,
    };
    response(state.journal_query().events(request, visibility).await)
}
async fn timeline(
    State(state): State<AppState>,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
    RawQuery(raw): RawQuery,
) -> Response {
    let Ok(Path(raw_id)) = path else {
        return failure(QueryError::Invalid);
    };
    let Some(device_id) = DeviceId::parse(&raw_id) else {
        return failure(QueryError::Invalid);
    };
    let window = match query::window(raw.as_deref()) {
        Ok(window) => window,
        Err(error) => return failure(error),
    };
    response(
        state
            .journal_query()
            .timeline(device_id, window.from, window.to)
            .await,
    )
}
async fn summaries(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Response {
    let window = match query::summary_window(raw.as_deref()) {
        Ok(window) => window,
        Err(error) => return failure(error),
    };
    response(state.journal_query().summaries(window).await)
}
async fn status(State(state): State<AppState>) -> Response {
    response(state.journal_query().status().await)
}

/// Bounded in-memory transfer buffer; body backpressure suspends only this read-only export.
const EXPORT_BUFFER_BYTES: usize = 64 * 1024;
async fn export(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Response {
    let request: ReadRequest = match query::events(raw.as_deref()) {
        Ok(request) => request,
        Err(error) => return failure(error),
    };
    let mut export = match state.journal_query().export(request).await {
        Ok(export) => export,
        Err(error) => return failure(error),
    };
    let (mut writer, reader) = tokio::io::duplex(EXPORT_BUFFER_BYTES);
    tokio::spawn(async move {
        loop {
            match export.next_line().await {
                Ok(Some(line)) => {
                    if writer.write_all(&line).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(error = %snafu::Report::from_error(error), "journal export interrupted");
                    if let Ok(line) = crate::journal_query::export_error_line() {
                        let _ = writer.write_all(&line).await;
                    }
                    break;
                }
            }
        }
    });
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"natsume-journal.jsonl\"",
            ),
        ],
        Body::from_stream(FramedRead::new(reader, BytesCodec::new())),
    )
        .into_response()
}

#[cfg(test)]
mod tests;
