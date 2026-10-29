use std::sync::{Arc, Mutex, MutexGuard};

use axum::{
    extract::{FromRequestParts as _, MatchedPath, RawPathParams, Request, State},
    http::{Method, StatusCode, request::Parts},
    middleware::Next,
    response::Response,
};
use uuid::Uuid;

use super::super::error::{ApiErrorCode, ApiErrorMetadata};
use crate::{
    component::operator::OperatorIdentity,
    journal::{
        Journal, JournalContext,
        event::{
            Bounded, Event,
            payload::OperatorRequest,
            types::{
                AuditCause, AuditRoute, AuditTarget, ErrorToken, HttpMethod, OperationId, RequestId,
            },
        },
    },
};

mod route;
pub(in crate::http) use route::{canonical_uuid, classify};

/// Keeps request targets below the details byte bound even for batch submissions.
const MAX_TARGETS: usize = 16;

#[derive(Clone)]
pub(in crate::http) struct RequestAudit {
    /// Server-created request UUID shared by request, domain, and auth records.
    request_id: RequestId,
    /// Valid request-span trace captured once at entry; None without a valid `OTel` context.
    trace_id: Option<opentelemetry::trace::TraceId>,
    /// Closed classification of an operator route; unclassified requests bypass audit.
    route: AuditRoute,
    /// Closed HTTP method classification, including unsupported methods.
    method: HttpMethod,
    /// Authentication and handlers update these bounded facts before completion.
    facts: Arc<Mutex<AuditFacts>>,
}

#[derive(Clone, Default)]
struct AuditFacts {
    /// Authentication supplies the live identity; None until authentication succeeds.
    actor: Option<OperatorIdentity>,
    /// Canonical route IDs and committed result IDs; deduplicated and capped at `MAX_TARGETS`.
    targets: Vec<AuditTarget>,
    /// Validated submission operation UUID supplied by its handler, otherwise None.
    operation_id: Option<OperationId>,
}

impl RequestAudit {
    async fn from_parts(parts: &mut Parts) -> Option<Self> {
        let route = parts
            .extensions
            .get::<MatchedPath>()
            .and_then(|path| classify(path.as_str()))?;
        let method = match parts.method {
            Method::POST => HttpMethod::Post,
            Method::PUT => HttpMethod::Put,
            Method::PATCH => HttpMethod::Patch,
            Method::DELETE => HttpMethod::Delete,
            Method::OPTIONS => HttpMethod::Options,
            Method::CONNECT => HttpMethod::Connect,
            Method::TRACE => HttpMethod::Trace,
            _ => HttpMethod::Other,
        };
        let audit = Self {
            request_id: RequestId::from_request(Uuid::now_v7()),
            route,
            trace_id: JournalContext::default().with_current_trace().trace_id(),
            method,
            facts: Arc::new(Mutex::new(AuditFacts::default())),
        };
        if let Ok(params) = RawPathParams::from_request_parts(parts, &()).await
            && let Some(target) = route::path_target(route, &params)
        {
            audit.record_target(target);
        }
        Some(audit)
    }

    pub(in crate::http) fn context(&self) -> JournalContext {
        self.context_with(&self.lock())
    }

    fn context_with(&self, facts: &AuditFacts) -> JournalContext {
        let context = facts
            .actor
            .as_ref()
            .map_or_else(JournalContext::default, JournalContext::for_operator)
            .with_request(self.request_id);
        let context = match self.trace_id {
            Some(trace_id) => context.with_trace_id(trace_id),
            None => context,
        };
        match facts.operation_id {
            Some(id) => context.with_operation(id),
            None => context,
        }
    }

    pub(in crate::http) fn record_actor(&self, identity: OperatorIdentity) {
        self.lock().actor = Some(identity);
    }

    pub(in crate::http) fn record_target(&self, target: AuditTarget) {
        let mut facts = self.lock();
        if facts.targets.len() < MAX_TARGETS && !facts.targets.contains(&target) {
            facts.targets.push(target);
        }
    }

    pub(in crate::http) fn record_operation(&self, operation_id: Uuid) {
        self.lock().operation_id = Some(OperationId::from_operation(operation_id));
    }

    fn lock(&self) -> MutexGuard<'_, AuditFacts> {
        // Only independent typed assignments and bounded deduplication occur here; poisoning cannot leave a partial business transition.
        self.facts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn emit(self, journal: &Journal, outcome: AuditOutcome) {
        let facts = self.lock().clone();
        let context = self.context_with(&facts);
        let targets = match Bounded::from_typed_targets(facts.targets) {
            Ok(targets) => targets,
            Err(error) => {
                tracing::error!(error = %snafu::Report::from_error(error), "request audit target bound failed");
                return;
            }
        };
        let api_code = outcome.error.and_then(|error| {
            ErrorToken::from_static_api_code(error.code.as_str())
                .inspect_err(|error| tracing::error!(error = %snafu::Report::from_error(error), "request audit API code is invalid"))
                .ok()
        });
        let cause = outcome.error.and_then(|error| {
            AuditCause::from_static_api_cause(error.cause)
                .inspect_err(|error| tracing::error!(error = %snafu::Report::from_error(error), "request audit cause is invalid"))
                .ok()
        });
        journal.emit(
            Event::OperatorRequest(OperatorRequest {
                route: self.route,
                method: self.method,
                targets,
                status: outcome.status.as_u16(),
                api_code,
                cause,
            }),
            context,
        );
    }
}

#[derive(Clone, Copy)]
struct AuditOutcome {
    /// HTTP status emitted by the completed response, or 500 if the task unwinds.
    status: StatusCode,
    /// Static metadata from `ApiError`, absent for successful or framework responses.
    error: Option<ApiErrorMetadata>,
}

impl AuditOutcome {
    fn response(response: &Response) -> Self {
        Self {
            status: response.status(),
            error: response.extensions().get().copied(),
        }
    }
}

struct PendingAudit {
    /// Taken on normal completion; a remaining value records one failed task on drop.
    audit: Option<RequestAudit>,
    /// Shared emit-only handle, without writer ownership or database access.
    journal: Journal,
}

impl PendingAudit {
    fn complete(mut self, response: &Response) {
        if let Some(audit) = self.audit.take() {
            audit.emit(&self.journal, AuditOutcome::response(response));
        }
    }
}

impl Drop for PendingAudit {
    fn drop(&mut self) {
        if let Some(audit) = self.audit.take() {
            audit.emit(
                &self.journal,
                AuditOutcome {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    error: Some(ApiErrorMetadata {
                        code: ApiErrorCode::InternalError,
                        cause: "request_task_failed",
                    }),
                },
            );
        }
    }
}

pub(in crate::http) async fn audit_request(
    State(journal): State<Journal>,
    request: Request,
    next: Next,
) -> Response {
    if matches!(*request.method(), Method::GET | Method::HEAD) {
        return next.run(request).await;
    }
    let (mut parts, body) = request.into_parts();
    let Some(audit) = RequestAudit::from_parts(&mut parts).await else {
        return next.run(Request::from_parts(parts, body)).await;
    };
    parts.extensions.insert(audit.clone());
    let pending = PendingAudit {
        audit: Some(audit),
        journal,
    };
    let response = next.run(Request::from_parts(parts, body)).await;
    pending.complete(&response);
    response
}

pub(in crate::http) async fn refused(
    request: Request,
    response: Response,
    journal: &Journal,
) -> Response {
    let (mut parts, _) = request.into_parts();
    if let Some(audit) = RequestAudit::from_parts(&mut parts).await {
        audit.emit(journal, AuditOutcome::response(&response));
    }
    response
}

#[cfg(test)]
mod tests;
