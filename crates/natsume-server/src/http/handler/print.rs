mod dto;

pub(crate) use dto::{
    GenerationFailure, PrintAdmission, PrintDocument, PrintGenerationFailure, PrintRegistration,
    PrintRequest, PrintSource, PrintSourceIssued, PrintState,
};

use axum::{
    Extension, Json, Router,
    body::Bytes,
    extract::{
        DefaultBodyLimit, FromRequestParts, Path, Request, State,
        rejection::{BytesRejection, JsonRejection},
    },
    http::{StatusCode, header, request::Parts},
    middleware::{self as axum_middleware, Next},
    response::{IntoResponse as _, Response},
    routing::{MethodRouter, delete, get, post, put},
};
use serde::Deserialize;
use utoipa::ToSchema;

use super::super::{AppState, error::ApiError, middleware};
use crate::component::{
    operator::OperatorIdentity,
    print::{DOCUMENT_LIMIT, PrintError, SourceIdentity, canonical_id},
};

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    let source = Router::new()
        .route(
            "/print/requests",
            require_source(state.clone(), post(register)),
        )
        .route(
            "/print/requests/{request_id}",
            require_source(state.clone(), get(get_request)),
        )
        .route(
            "/print/requests/{request_id}/generation-failure",
            require_source(state.clone(), post(generation_failure)),
        )
        .route(
            "/print/requests/{request_id}/document",
            require_source(
                state.clone(),
                put(upload).layer(DefaultBodyLimit::max(DOCUMENT_LIMIT)),
            ),
        );
    source
        .route(
            "/print/sources",
            middleware::require_admin(state.clone(), post(create_source).get(list_sources)),
        )
        .route(
            "/print/sources/{source_id}",
            middleware::require_admin(state, delete(revoke_source)),
        )
}

fn require_source(state: AppState, routes: MethodRouter<AppState>) -> MethodRouter<AppState> {
    routes.route_layer(axum_middleware::from_fn_with_state(
        state,
        authenticate_source,
    ))
}

async fn authenticate_source(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let mut auth = headers.get_all(header::AUTHORIZATION).iter();
    let token = auth
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) = token.filter(|_| auth.next().is_none()) else {
        return ApiError::from_print(PrintError::Authentication).into_response();
    };
    match state.print().authenticate(token).await {
        Ok(identity) => {
            request.extensions_mut().insert(identity);
            next.run(request).await
        }
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintSourceCreate {
    #[schema(min_length = 1, max_length = 128)]
    name: String,
}

/// Canonical path IDs reject Axum decoding failures using the public error envelope.
pub(crate) struct PrintPathId(String);
impl<S: Send + Sync> FromRequestParts<S> for PrintPathId {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(id) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::invalid_request("print_path").into_response())?;
        canonical_id(&id).map_err(|error| ApiError::from_print(error).into_response())?;
        Ok(Self(id))
    }
}

/// Raw PDF bytes, not a JSON array.
struct PrintPdfBody;
impl utoipa::ToSchema for PrintPdfBody {}
impl utoipa::PartialSchema for PrintPdfBody {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::schema::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .format(Some(utoipa::openapi::SchemaFormat::KnownFormat(
                utoipa::openapi::schema::KnownFormat::Binary,
            )))
            .into()
    }
}

#[utoipa::path(post, path = "/api/v2/print/sources", operation_id = "createPrintSource", security(("sessionCookie" = [])), request_body = PrintSourceCreate,
    responses((status = 200, description = "Source credential returned only once", body = PrintSourceIssued), (status = 400, description = "Invalid fields"), (status = 401, description = "Authentication failed"), (status = 403, description = "Admin required"), (status = 500, description = "Persistence failed")))]
pub(crate) async fn create_source(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    body: Result<Json<PrintSourceCreate>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return ApiError::invalid_request("print_json").into_response();
    };
    match state.print().create_source(actor, body.name).await {
        Ok(source) => Json(PrintSourceIssued::from(source)).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

#[utoipa::path(get, path = "/api/v2/print/sources", operation_id = "listPrintSources", security(("sessionCookie" = [])),
    responses((status = 200, description = "Non-secret sources", body = [PrintSource]), (status = 401, description = "Authentication failed"), (status = 403, description = "Admin required"), (status = 500, description = "Persistence failed")))]
pub(crate) async fn list_sources(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
) -> Response {
    match state.print().list_sources(actor).await {
        Ok(sources) => Json(
            sources
                .into_iter()
                .map(PrintSource::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

#[utoipa::path(delete, path = "/api/v2/print/sources/{source_id}", operation_id = "revokePrintSource", security(("sessionCookie" = [])), params(("source_id" = String, Path)),
    responses((status = 204, description = "Source revoked idempotently"), (status = 400, description = "Invalid ID"), (status = 401, description = "Authentication failed"), (status = 403, description = "Admin required"), (status = 404, description = "Unknown source"), (status = 500, description = "Persistence failed")))]
pub(crate) async fn revoke_source(
    State(state): State<AppState>,
    Extension(actor): Extension<OperatorIdentity>,
    PrintPathId(id): PrintPathId,
) -> Response {
    match state.print().revoke_source(actor, id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::from_print(e).into_response(),
    }
}

#[utoipa::path(post, path = "/api/v2/print/requests", operation_id = "registerPrintRequest", security(("printSourceBearer" = [])), request_body = PrintRegistration,
    responses((status = 200, description = "Registered request, not yet queued", body = PrintRequest), (status = 400, description = "Invalid registration"), (status = 401, description = "Source authentication failed"), (status = 409, description = "Registration conflict"), (status = 500, description = "Persistence failed")))]
pub(crate) async fn register(
    State(state): State<AppState>,
    Extension(source): Extension<SourceIdentity>,
    body: Result<Json<PrintRegistration>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return ApiError::invalid_request("print_json").into_response();
    };
    match state.print().register(source, body.into()).await {
        Ok(request) => Json(PrintRequest::from(request)).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

#[utoipa::path(get, path = "/api/v2/print/requests/{request_id}", operation_id = "getPrintRequest", security(("printSourceBearer" = [])), params(("request_id" = String, Path)),
    responses((status = 200, description = "Current fact and original admission", body = PrintRequest), (status = 400, description = "Invalid ID"), (status = 401, description = "Source authentication failed"), (status = 404, description = "Request not found in this source"), (status = 500, description = "Persistence failed")))]
pub(crate) async fn get_request(
    State(state): State<AppState>,
    Extension(source): Extension<SourceIdentity>,
    PrintPathId(id): PrintPathId,
) -> Response {
    match state.print().get(source, id).await {
        Ok(request) => Json(PrintRequest::from(request)).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

#[utoipa::path(post, path = "/api/v2/print/requests/{request_id}/generation-failure", operation_id = "reportPrintGenerationFailure", security(("printSourceBearer" = [])), params(("request_id" = String, Path)), request_body = PrintGenerationFailure,
    responses((status = 200, description = "Durable generation failure", body = PrintRequest), (status = 400, description = "Invalid failure"), (status = 401, description = "Source authentication failed"), (status = 404, description = "Unknown request"), (status = 409, description = "Phase conflict"), (status = 500, description = "Persistence failed")))]
pub(crate) async fn generation_failure(
    State(state): State<AppState>,
    Extension(source): Extension<SourceIdentity>,
    PrintPathId(id): PrintPathId,
    body: Result<Json<PrintGenerationFailure>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return ApiError::invalid_request("print_json").into_response();
    };
    match state
        .print()
        .generation_failure(source, id, body.into())
        .await
    {
        Ok(request) => Json(PrintRequest::from(request)).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}

#[utoipa::path(put, path = "/api/v2/print/requests/{request_id}/document", operation_id = "uploadPrintDocument", security(("printSourceBearer" = [])), params(("request_id" = String, Path)),
    request_body(content = inline(PrintPdfBody), content_type = "application/pdf"),
    responses((status = 200, description = "PDF and first FIFO snapshot committed atomically", body = PrintRequest), (status = 400, description = "Invalid PDF"), (status = 401, description = "Source authentication failed"), (status = 404, description = "Unknown request"), (status = 409, description = "Phase, document or identity conflict"), (status = 413, description = "PDF exceeds 8 MiB"), (status = 500, description = "Persistence failed")))]
pub(crate) async fn upload(
    State(state): State<AppState>,
    Extension(source): Extension<SourceIdentity>,
    PrintPathId(id): PrintPathId,
    headers: axum::http::HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        != Some("application/pdf")
    {
        return ApiError::invalid_request("print_pdf_content_type").into_response();
    }
    let bytes = match body {
        Ok(bytes) => bytes,
        Err(e) if e.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return ApiError::from_print(PrintError::TooLarge).into_response();
        }
        Err(_) => return ApiError::invalid_request("print_pdf_body").into_response(),
    };
    match state.print().upload(source, id, bytes.to_vec()).await {
        Ok(request) => Json(PrintRequest::from(request)).into_response(),
        Err(error) => ApiError::from_print(error).into_response(),
    }
}
