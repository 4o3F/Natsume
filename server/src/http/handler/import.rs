use axum::{
    Extension, Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Request, State, rejection::BytesRejection},
    http::{StatusCode, header},
    middleware as axum_middleware,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tower_http::limit::RequestBodyLimitLayer;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{
    audit::CorrelationId,
    component::{
        import::{
            ImportBindingImpact, ImportMappingChange, PendingImportCandidate, PreviewToken,
            RedactedImportPreview,
        },
        operator::OperatorIdentity,
    },
};

use super::super::{AppState, error::ApiError, middleware, not_found};

pub(crate) const CSV_IMPORT_BODY_LIMIT_BYTES: usize = 4_194_304;
pub(crate) const IMPORT_COMMIT_BODY_LIMIT_BYTES: usize = CSV_IMPORT_BODY_LIMIT_BYTES + 4_096;

const PREVIEW_TOKEN_BYTES: usize = 32;
const PREVIEW_TOKEN_WIRE_LENGTH: usize = 43;

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    let upload = post(create_import)
        .layer(DefaultBodyLimit::max(CSV_IMPORT_BODY_LIMIT_BYTES))
        .route_layer(axum_middleware::from_fn(require_csv_content_type))
        .route_layer(axum_middleware::from_fn(require_admin_role));
    let upload = middleware::require_operator(state.clone(), upload)
        .layer(RequestBodyLimitLayer::new(CSV_IMPORT_BODY_LIMIT_BYTES));
    let commit = post(commit_import)
        .layer(DefaultBodyLimit::max(IMPORT_COMMIT_BODY_LIMIT_BYTES))
        .route_layer(axum_middleware::from_fn(require_admin_role));
    let discard = post(discard_import).route_layer(axum_middleware::from_fn(require_admin_role));
    let read = get(get_import)
        .route_layer(axum_middleware::from_fn(require_admin_role))
        .head(not_found);
    let imports = upload.merge(middleware::require_operator(state.clone(), read));
    Router::new()
        .route("/imports", imports)
        .route(
            "/imports/{import_id}/actions/commit",
            middleware::require_operator(state.clone(), commit),
        )
        .route(
            "/imports/{import_id}/actions/discard",
            middleware::require_operator(state, discard),
        )
}

async fn require_admin_role(
    Extension(correlation_id): Extension<CorrelationId>,
    Extension(identity): Extension<OperatorIdentity>,
    request: Request,
    next: Next,
) -> Response {
    if let Err(error) = identity.require_admin() {
        return ApiError::from_operator(error, correlation_id).into_response();
    }
    next.run(request).await
}

async fn require_csv_content_type(
    Extension(correlation_id): Extension<CorrelationId>,
    request: Request,
    next: Next,
) -> Response {
    let content_type_is_csv = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/csv"));
    if !content_type_is_csv {
        return ApiError::invalid_request("import_content_type_rejected", correlation_id)
            .into_response();
    }
    next.run(request).await
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct ImportPath {
    /// Canonical lowercase hyphenated `UUIDv7` import candidate ID.
    import_id: String,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportMappingChangeResponse {
    seat_code: String,
    #[schema(required = true)]
    current_domjudge_username: Option<String>,
    candidate_domjudge_username: String,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportBindingImpactResponse {
    seat_code: String,
    device_id: String,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportRedactedDiff {
    seats_added: Vec<String>,
    seats_removed: Vec<String>,
    mappings_changed: Vec<ImportMappingChangeResponse>,
    unchanged_count: usize,
    affected_account_count: usize,
    binding_impacts: Vec<ImportBindingImpactResponse>,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportPreviewResponse {
    candidate_id: Uuid,
    #[schema(
        min_length = 43,
        max_length = 43,
        pattern = "^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$"
    )]
    preview_token: String,
    expires_at_unix_ms: i64,
    diff: ImportRedactedDiff,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportPendingSummary {
    candidate_id: Uuid,
    expires_at_unix_ms: i64,
    diff: ImportRedactedDiff,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportPendingResponse {
    #[schema(required = true)]
    pending: Option<ImportPendingSummary>,
}

#[derive(Deserialize, ToSchema, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportCommitRequest {
    #[schema(
        write_only,
        min_length = 43,
        max_length = 43,
        pattern = "^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$"
    )]
    preview_token: String,
    /// The same seat/account candidate reviewed in preview, with the passwords
    /// supplied again because preview never persists them.
    #[schema(write_only)]
    csv: String,
}

#[utoipa::path(
    post,
    path = "/api/v2/imports",
    operation_id = "createCsvImport",
    security(("sessionCookie" = [])),
    request_body(content = String, content_type = "text/csv"),
    responses(
        (status = 201, description = "CSV import candidate created", body = ImportPreviewResponse),
        (status = 400, description = "Invalid CSV import or request media type"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 409, description = "An import candidate is already pending"),
        (status = 413, description = "CSV request body exceeds the import ingress limit"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn create_import(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let body = match body {
        Ok(body) => body,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return rejection.into_response();
        }
        Err(_) => {
            return ApiError::invalid_request("import_request_body_rejected", correlation_id)
                .into_response();
        }
    };
    match state.import().create_candidate(&body, correlation_id).await {
        Ok(created) => {
            let response = ImportPreviewResponse {
                candidate_id: created.candidate_id(),
                preview_token: encode_preview_token(created.preview_token().as_bytes()),
                expires_at_unix_ms: created.expires_at_unix_ms(),
                diff: ImportRedactedDiff::from(created.diff()),
            };
            json_response(StatusCode::CREATED, &response, correlation_id)
        }
        Err(error) => ApiError::from_import(error, correlation_id).into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/imports",
    operation_id = "getCsvImport",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current pending CSV import candidate", body = ImportPendingResponse),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn get_import(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
) -> Response {
    match state.import().read_pending(correlation_id).await {
        Ok(pending) => json_response(
            StatusCode::OK,
            &ImportPendingResponse {
                pending: pending.as_ref().map(ImportPendingSummary::from),
            },
            correlation_id,
        ),
        Err(error) => ApiError::from_import(error, correlation_id).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/imports/{import_id}/actions/commit",
    operation_id = "commitCsvImport",
    params(ImportPath),
    security(("sessionCookie" = [])),
    request_body = ImportCommitRequest,
    responses(
        (status = 204, description = "CSV import committed"),
        (status = 400, description = "Invalid import ID or closed request"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Import candidate unavailable"),
        (status = 409, description = "Import preview baseline is stale"),
        (status = 413, description = "Commit request body exceeds the import ingress limit"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn commit_import(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Path(path): Path<ImportPath>,
    request: Result<Json<ImportCommitRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some(import_id) = canonical_uuid_v7(&path.import_id) else {
        return ApiError::invalid_request("import_id_not_canonical_uuid_v7", correlation_id)
            .into_response();
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return rejection.into_response();
        }
        Err(_) => {
            return ApiError::invalid_request("import_commit_body_rejected", correlation_id)
                .into_response();
        }
    };
    let Some(token) = decode_preview_token(&request.preview_token) else {
        return ApiError::invalid_request("import_preview_token_rejected", correlation_id)
            .into_response();
    };
    match state
        .import()
        .commit(
            import_id,
            &PreviewToken::from_bytes(token),
            request.csv.as_bytes(),
            correlation_id,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_import(error, correlation_id).into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/imports/{import_id}/actions/discard",
    operation_id = "discardCsvImport",
    params(ImportPath),
    security(("sessionCookie" = [])),
    responses(
        (status = 204, description = "CSV import candidate discarded"),
        (status = 400, description = "Invalid import ID"),
        (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"),
        (status = 404, description = "Import candidate unavailable"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn discard_import(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    Path(path): Path<ImportPath>,
) -> Response {
    let Some(import_id) = canonical_uuid_v7(&path.import_id) else {
        return ApiError::invalid_request("import_id_not_canonical_uuid_v7", correlation_id)
            .into_response();
    };
    match state.import().discard(import_id, correlation_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_import(error, correlation_id).into_response(),
    }
}

impl From<&RedactedImportPreview> for ImportRedactedDiff {
    fn from(diff: &RedactedImportPreview) -> Self {
        Self {
            seats_added: diff.seats_added().to_vec(),
            seats_removed: diff.seats_removed().to_vec(),
            mappings_changed: diff
                .mappings_changed()
                .iter()
                .map(ImportMappingChangeResponse::from)
                .collect(),
            unchanged_count: diff.unchanged_count(),
            affected_account_count: diff.affected_account_count(),
            binding_impacts: diff
                .binding_impacts()
                .iter()
                .map(ImportBindingImpactResponse::from)
                .collect(),
        }
    }
}

impl From<&ImportMappingChange> for ImportMappingChangeResponse {
    fn from(change: &ImportMappingChange) -> Self {
        Self {
            seat_code: change.seat_code().to_owned(),
            current_domjudge_username: change.current_domjudge_username().map(str::to_owned),
            candidate_domjudge_username: change.candidate_domjudge_username().to_owned(),
        }
    }
}

impl From<&ImportBindingImpact> for ImportBindingImpactResponse {
    fn from(impact: &ImportBindingImpact) -> Self {
        Self {
            seat_code: impact.seat_code().to_owned(),
            device_id: impact.device_id().to_owned(),
        }
    }
}

impl From<&PendingImportCandidate> for ImportPendingSummary {
    fn from(pending: &PendingImportCandidate) -> Self {
        Self {
            candidate_id: pending.candidate_id(),
            expires_at_unix_ms: pending.expires_at_unix_ms(),
            diff: ImportRedactedDiff::from(pending.diff()),
        }
    }
}

fn canonical_uuid_v7(value: &str) -> Option<Uuid> {
    let parsed = Uuid::parse_str(value).ok()?;
    (parsed.get_version_num() == 7 && parsed.hyphenated().to_string() == value).then_some(parsed)
}

fn encode_preview_token(token: &[u8; PREVIEW_TOKEN_BYTES]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token)
}

fn decode_preview_token(value: &str) -> Option<[u8; PREVIEW_TOKEN_BYTES]> {
    // The explicit wire-length guard stays in front of the engine: it pins the frozen
    // 43-character token shape independently of the decoder's own length arithmetic.
    if value.len() != PREVIEW_TOKEN_WIRE_LENGTH {
        return None;
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .ok()?
        .try_into()
        .ok()
}

fn json_response<T: Serialize>(
    status: StatusCode,
    body: &T,
    correlation_id: CorrelationId,
) -> Response {
    let encoded = serde_json::to_vec(body).unwrap_or_else(|_| {
        tracing::error!(
            correlation_id = %correlation_id.as_text(),
            "import response serialization invariant failed"
        );
        panic!("import response serialization invariant failed");
    });
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        encoded,
    )
        .into_response()
}
