use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware as axum_middleware,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::super::{AppState, error::ApiError, middleware};
use crate::component::import::{
    OrganizationDetails, PendingImportCandidate, RedactedImportPreview, TeamDetails,
};

const PREVIEW_TOKEN_BYTES: usize = 32;
const PREVIEW_TOKEN_WIRE_LENGTH: usize = 43;
const XLSX_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const PREVIEW_TOKEN_HEADER: &str = "x-natsume-preview-token";

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    let upload =
        post(create_import).route_layer(axum_middleware::from_fn(require_xlsx_content_type));
    let commit =
        post(commit_import).route_layer(axum_middleware::from_fn(require_xlsx_content_type));
    Router::new()
        .route(
            "/imports",
            middleware::require_admin(state.clone(), upload.merge(get(get_import))),
        )
        .route(
            "/imports/template",
            middleware::require_admin(state.clone(), get(get_template)),
        )
        .route(
            "/imports/{import_id}/actions/commit",
            middleware::require_admin(state.clone(), commit),
        )
        .route(
            "/imports/{import_id}",
            middleware::require_admin(state, delete(delete_import)),
        )
        .layer(DefaultBodyLimit::max(natsume_roster::MAX_WORKBOOK_BYTES))
}

async fn require_xlsx_content_type(request: Request, next: Next) -> Response {
    let accepted = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(XLSX_CONTENT_TYPE));
    if !accepted {
        return ApiError::invalid_request("import_content_type_rejected").into_response();
    }
    next.run(request).await
}

/// Raw XLSX bytes, never a JSON array or a persisted upload.
struct RosterWorkbook;
impl utoipa::ToSchema for RosterWorkbook {}
impl utoipa::PartialSchema for RosterWorkbook {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::schema::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .format(Some(utoipa::openapi::SchemaFormat::KnownFormat(
                utoipa::openapi::schema::KnownFormat::Binary,
            )))
            .into()
    }
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
    blocks_commit: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportOrganizationResponse {
    organization_id: String,
    name_zh: String,
    name_en: String,
    country: String,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportTeamResponse {
    account: String,
    #[schema(required = true)]
    seat: Option<String>,
    organization_id: String,
    name_zh: String,
    name_en: String,
    category: String,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportOrganizationChangeResponse {
    #[schema(required = true)]
    current: Option<ImportOrganizationResponse>,
    #[schema(required = true)]
    candidate: Option<ImportOrganizationResponse>,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportTeamChangeResponse {
    account: String,
    #[schema(required = true)]
    current: Option<ImportTeamResponse>,
    #[schema(required = true)]
    candidate: Option<ImportTeamResponse>,
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
    accounts_added: Vec<String>,
    accounts_removed: Vec<String>,
    passwords_changed: Vec<String>,
    organizations: Vec<ImportOrganizationResponse>,
    organization_changes: Vec<ImportOrganizationChangeResponse>,
    team_changes: Vec<ImportTeamChangeResponse>,
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

#[utoipa::path(get, path = "/api/v2/imports/template", operation_id = "getRosterTemplate",
    security(("sessionCookie" = [])),
    responses((status = 200, description = "Blank Teams XLSX template", body = inline(RosterWorkbook), content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        (status = 401, description = "Session authentication failed"), (status = 403, description = "Administrator role required")))]
pub(crate) async fn get_template() -> Response {
    (
        [
            (header::CONTENT_TYPE, XLSX_CONTENT_TYPE),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=roster-template.xlsx",
            ),
        ],
        include_bytes!("../../../../crates/roster/examples/template.xlsx").as_slice(),
    )
        .into_response()
}

#[utoipa::path(post, path = "/api/v2/imports", operation_id = "createRosterImport",
    security(("sessionCookie" = [])),
    request_body(content = inline(RosterWorkbook), content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
    responses((status = 201, description = "Full-roster XLSX preview created", body = ImportPreviewResponse),
        (status = 400, description = "Invalid workbook or media type"), (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"), (status = 409, description = "A candidate is already pending"),
        (status = 413, description = "Workbook exceeds 8 MiB"), (status = 500, description = "Internal failure")))]
pub(crate) async fn create_import(State(state): State<AppState>, body: Bytes) -> Response {
    match state.import().create_candidate(&body).await {
        Ok(created) => (
            StatusCode::CREATED,
            Json(ImportPreviewResponse {
                candidate_id: created.candidate_id(),
                preview_token: encode_preview_token(created.preview_token_bytes()),
                expires_at_unix_ms: created.expires_at_unix_ms(),
                diff: ImportRedactedDiff::from(created.diff()),
            }),
        )
            .into_response(),
        Err(error) => ApiError::from_import(error).into_response(),
    }
}

#[utoipa::path(get, path = "/api/v2/imports", operation_id = "getRosterImport", security(("sessionCookie" = [])),
    responses((status = 200, description = "Pending roster preview", body = ImportPendingResponse),
        (status = 401, description = "Session authentication failed"), (status = 403, description = "Administrator role required"), (status = 500, description = "Internal failure")))]
pub(crate) async fn get_import(State(state): State<AppState>) -> Response {
    match state.import().read_pending().await {
        Ok(pending) => Json(ImportPendingResponse {
            pending: pending.as_ref().map(ImportPendingSummary::from),
        })
        .into_response(),
        Err(error) => ApiError::from_import(error).into_response(),
    }
}

#[utoipa::path(post, path = "/api/v2/imports/{import_id}/actions/commit", operation_id = "commitRosterImport", params(ImportPath,
    ("x-natsume-preview-token" = String, Header, description = "Secret authorization returned by preview", min_length = 43, max_length = 43, pattern = "^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$")),
    security(("sessionCookie" = [])),
    request_body(content = inline(RosterWorkbook), content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
    responses((status = 204, description = "Complete roster committed atomically; only changed passwords advance revisions"),
        (status = 400, description = "Invalid workbook, token or media type"), (status = 401, description = "Session authentication failed"),
        (status = 403, description = "Administrator role required"), (status = 404, description = "Candidate unavailable"),
        (status = 409, description = "Preview stale or removed seat occupied"), (status = 413, description = "Workbook exceeds 8 MiB"), (status = 500, description = "Internal failure")))]
pub(crate) async fn commit_import(
    State(state): State<AppState>,
    Path(path): Path<ImportPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(import_id) = canonical_uuid_v7(&path.import_id) else {
        return ApiError::invalid_request("import_id_not_canonical_uuid_v7").into_response();
    };
    let Some(token) = headers
        .get(PREVIEW_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(decode_preview_token)
    else {
        return ApiError::invalid_request("import_preview_token_rejected").into_response();
    };
    match state.import().commit(import_id, &token, &body).await {
        Ok(()) => {
            state.device_control().dirty_all_devices().await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => ApiError::from_import(error).into_response(),
    }
}

#[utoipa::path(delete, path = "/api/v2/imports/{import_id}", operation_id = "deleteRosterImport", params(ImportPath), security(("sessionCookie" = [])),
    responses((status = 204, description = "Roster preview discarded"), (status = 400, description = "Invalid import ID"),
        (status = 401, description = "Session authentication failed"), (status = 403, description = "Administrator role required"),
        (status = 404, description = "Candidate unavailable"), (status = 500, description = "Internal failure")))]
pub(crate) async fn delete_import(
    State(state): State<AppState>,
    Path(path): Path<ImportPath>,
) -> Response {
    let Some(import_id) = canonical_uuid_v7(&path.import_id) else {
        return ApiError::invalid_request("import_id_not_canonical_uuid_v7").into_response();
    };
    match state.import().discard(import_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from_import(error).into_response(),
    }
}

impl From<&OrganizationDetails> for ImportOrganizationResponse {
    fn from(value: &OrganizationDetails) -> Self {
        Self {
            organization_id: format!("INST-{:03}", value.organization_id),
            name_zh: value.name_zh.clone(),
            name_en: value.name_en.clone(),
            country: value.country.clone(),
        }
    }
}
impl From<&TeamDetails> for ImportTeamResponse {
    fn from(value: &TeamDetails) -> Self {
        Self {
            account: value.account.clone(),
            seat: value.seat.clone(),
            organization_id: format!("INST-{:03}", value.organization_id),
            name_zh: value.name_zh.clone(),
            name_en: value.name_en.clone(),
            category: value.category.clone(),
        }
    }
}
impl From<&RedactedImportPreview> for ImportRedactedDiff {
    fn from(diff: &RedactedImportPreview) -> Self {
        Self {
            seats_added: diff.seats_added.clone(),
            seats_removed: diff.seats_removed.clone(),
            mappings_changed: diff
                .mappings_changed
                .iter()
                .map(|change| ImportMappingChangeResponse {
                    seat_code: change.seat_code.clone(),
                    current_domjudge_username: change.current_domjudge_username.clone(),
                    candidate_domjudge_username: change.candidate_domjudge_username.clone(),
                })
                .collect(),
            unchanged_count: diff.unchanged_count,
            affected_account_count: diff.affected_account_count,
            binding_impacts: diff
                .binding_impacts
                .iter()
                .map(|impact| ImportBindingImpactResponse {
                    seat_code: impact.seat_code.clone(),
                    device_id: impact.device_id.clone(),
                    blocks_commit: impact.blocks_commit,
                })
                .collect(),
            accounts_added: diff.accounts_added.clone(),
            accounts_removed: diff.accounts_removed.clone(),
            passwords_changed: diff.passwords_changed.clone(),
            organizations: diff
                .organizations
                .iter()
                .map(ImportOrganizationResponse::from)
                .collect(),
            organization_changes: diff
                .organization_changes
                .iter()
                .map(|change| ImportOrganizationChangeResponse {
                    current: change
                        .current
                        .as_ref()
                        .map(ImportOrganizationResponse::from),
                    candidate: change
                        .candidate
                        .as_ref()
                        .map(ImportOrganizationResponse::from),
                })
                .collect(),
            team_changes: diff
                .team_changes
                .iter()
                .map(|change| ImportTeamChangeResponse {
                    account: change.account.clone(),
                    current: change.current.as_ref().map(ImportTeamResponse::from),
                    candidate: change.candidate.as_ref().map(ImportTeamResponse::from),
                })
                .collect(),
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
