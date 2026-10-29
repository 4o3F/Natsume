use crate::http::Routes;
use crate::http::middleware::audit::RequestAudit;
use axum::Extension;

use axum::{
    Json,
    body::Bytes,
    extract::{FromRequest, Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware as axum_middleware,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::{AppState, error::ApiError, middleware};
use crate::component::import::{
    ImportOperation, OrganizationDetails, PendingImportCandidate, RedactedImportPreview,
    TeamDetails,
};

pub(crate) struct RosterUpload {
    operation: ImportOperation,
    bytes: Bytes,
}

impl FromRequest<AppState> for RosterUpload {
    type Rejection = Response;

    async fn from_request(request: Request, state: &AppState) -> Result<Self, Self::Rejection> {
        let operation = state
            .import()
            .try_begin()
            .map_err(|error| ApiError::from_import(error).into_response())?;
        let bytes = Bytes::from_request(request, state)
            .await
            .map_err(IntoResponse::into_response)?;
        Ok(Self { operation, bytes })
    }
}

const PREVIEW_TOKEN_BYTES: usize = 32;
const PREVIEW_TOKEN_WIRE_LENGTH: usize = 43;
const XLSX_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const PREVIEW_TOKEN_HEADER: &str = "x-natsume-preview-token";

pub(in crate::http) fn operator_routes() -> Routes {
    let upload =
        post(create_import).route_layer(axum_middleware::from_fn(require_xlsx_content_type));
    let commit =
        post(commit_import).route_layer(axum_middleware::from_fn(require_xlsx_content_type));
    vec![
        (
            "/imports",
            middleware::require_admin(upload.merge(get(get_import))),
        ),
        (
            "/imports/template",
            middleware::require_admin(get(get_template)),
        ),
        (
            "/imports/{import_id}/actions/commit",
            middleware::require_admin(commit),
        ),
        (
            "/imports/{import_id}",
            middleware::require_admin(delete(delete_import)),
        ),
    ]
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

#[derive(Deserialize)]
pub(crate) struct ImportPath {
    /// Canonical lowercase hyphenated `UUIDv7` import candidate ID.
    import_id: String,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportMappingChangeResponse {
    seat_code: String,
    current_domjudge_username: Option<String>,
    candidate_domjudge_username: String,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportBindingImpactResponse {
    seat_code: String,
    device_id: String,
    blocks_commit: bool,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportOrganizationResponse {
    organization_id: String,
    name_zh: String,
    name_en: String,
    country: String,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportTeamResponse {
    account: String,
    seat: Option<String>,
    organization_id: String,
    name_zh: String,
    name_en: String,
    category: String,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportOrganizationChangeResponse {
    current: Option<ImportOrganizationResponse>,
    candidate: Option<ImportOrganizationResponse>,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportTeamChangeResponse {
    account: String,
    current: Option<ImportTeamResponse>,
    candidate: Option<ImportTeamResponse>,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportRedactedDiff {
    seats_added: Vec<String>,
    seats_removed: Vec<String>,
    mappings_changed: Vec<ImportMappingChangeResponse>,
    #[specta(type = specta_typescript::Number)]
    unchanged_count: usize,
    #[specta(type = specta_typescript::Number)]
    affected_account_count: usize,
    binding_impacts: Vec<ImportBindingImpactResponse>,
    accounts_added: Vec<String>,
    accounts_removed: Vec<String>,
    passwords_changed: Vec<String>,
    organizations: Vec<ImportOrganizationResponse>,
    organization_changes: Vec<ImportOrganizationChangeResponse>,
    team_changes: Vec<ImportTeamChangeResponse>,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportPreviewResponse {
    candidate_id: Uuid,
    preview_token: String,
    #[specta(type = specta_typescript::Number)]
    expires_at_unix_ms: i64,
    diff: ImportRedactedDiff,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportPendingSummary {
    candidate_id: Uuid,
    #[specta(type = specta_typescript::Number)]
    expires_at_unix_ms: i64,
    diff: ImportRedactedDiff,
}

#[derive(Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportPendingResponse {
    pending: Option<ImportPendingSummary>,
}

pub(crate) async fn get_template() -> Response {
    (
        [
            (header::CONTENT_TYPE, XLSX_CONTENT_TYPE),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=roster-template.xlsx",
            ),
        ],
        crate::component::import::XLSX_TEMPLATE,
    )
        .into_response()
}

pub(in crate::http) async fn create_import(
    Extension(audit): Extension<RequestAudit>,
    upload: RosterUpload,
) -> Response {
    match upload.operation.create_candidate(&upload.bytes).await {
        Ok(created) => {
            audit.record_target(crate::journal::event::types::AuditTarget::Import(
                created.candidate_id(),
            ));
            (
                StatusCode::CREATED,
                Json(ImportPreviewResponse {
                    candidate_id: created.candidate_id(),
                    preview_token: encode_preview_token(created.preview_token_bytes()),
                    expires_at_unix_ms: created.expires_at_unix_ms(),
                    diff: ImportRedactedDiff::from(created.diff()),
                }),
            )
                .into_response()
        }
        Err(error) => ApiError::from_import(error).into_response(),
    }
}

pub(crate) async fn get_import(State(state): State<AppState>) -> Response {
    match state.import().read_pending().await {
        Ok(pending) => Json(ImportPendingResponse {
            pending: pending.as_ref().map(ImportPendingSummary::from),
        })
        .into_response(),
        Err(error) => ApiError::from_import(error).into_response(),
    }
}

pub(crate) async fn commit_import(
    State(state): State<AppState>,
    Path(path): Path<ImportPath>,
    headers: HeaderMap,
    upload: RosterUpload,
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
    match upload
        .operation
        .commit(import_id, &token, &upload.bytes)
        .await
    {
        Ok(change) => {
            state.device_control().publish(change).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => ApiError::from_import(error).into_response(),
    }
}

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
