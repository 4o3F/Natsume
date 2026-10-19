use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use super::{super::super::error::ApiError, AppState, middleware};
use crate::component::contest::{LogoObservation, LogoStatus, OrganizationDetails};

pub(super) fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/organizations",
            middleware::require_admin(state.clone(), get(list_organizations)),
        )
        .route(
            "/imports/{import_id}/organizations",
            middleware::require_admin(state.clone(), get(list_candidate_organizations)),
        )
        .route(
            "/imports/{import_id}/organizations/{organization_id}/logo",
            middleware::require_admin(state.clone(), get(candidate_logo)),
        )
        .route(
            "/exports/domjudge",
            middleware::require_admin(state, get(export_domjudge)),
        )
        // School logos are public images; this route never exposes a roster or password.
        .route(
            "/organizations/{organization_id}/logo",
            get(organization_logo),
        )
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OrganizationLogoStatus {
    Available,
    Missing,
    Ambiguous,
    Invalid,
}
#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrganizationLogoResponse {
    organization_id: String,
    name_zh: String,
    name_en: String,
    country: String,
    status: OrganizationLogoStatus,
    files: Vec<String>,
    #[schema(required = true)]
    detail: Option<String>,
}
impl From<LogoObservation> for OrganizationLogoResponse {
    fn from(value: LogoObservation) -> Self {
        Self {
            organization_id: format!("INST-{:03}", value.organization.organization_id),
            name_zh: value.organization.name_zh,
            name_en: value.organization.name_en,
            country: value.organization.country,
            status: match value.status {
                LogoStatus::Available => OrganizationLogoStatus::Available,
                LogoStatus::Missing => OrganizationLogoStatus::Missing,
                LogoStatus::Ambiguous => OrganizationLogoStatus::Ambiguous,
                LogoStatus::Invalid => OrganizationLogoStatus::Invalid,
            },
            files: value.files,
            detail: value.detail,
        }
    }
}

struct Download;
impl utoipa::ToSchema for Download {}
impl utoipa::PartialSchema for Download {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::schema::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .format(Some(utoipa::openapi::SchemaFormat::KnownFormat(
                utoipa::openapi::schema::KnownFormat::Binary,
            )))
            .into()
    }
}

#[utoipa::path(get, path = "/api/v2/organizations", operation_id = "listOrganizationLogos", security(("sessionCookie" = [])), responses(
    (status = 200, description = "All committed schools and current source image status", body = [OrganizationLogoResponse]),
    (status = 401, description = "Authentication required"), (status = 403, description = "Administrator required"), (status = 500, description = "Cannot read schools or logo directory"), (status = 503, description = "Image workers busy")
))]
pub(crate) async fn list_organizations(State(state): State<AppState>) -> Response {
    let schools = match state.contest().list_organizations().await {
        Ok(schools) => schools,
        Err(error) => return ApiError::from_contest(error).into_response(),
    };
    observations(&state, schools).await
}

#[utoipa::path(get, path = "/api/v2/imports/{import_id}/organizations", operation_id = "listCandidateOrganizationLogos", security(("sessionCookie" = [])), params(("import_id" = String, Path)), responses(
    (status = 200, description = "Pending school ID mapping and current source image status", body = [OrganizationLogoResponse]),
    (status = 401, description = "Authentication required"), (status = 403, description = "Administrator required"), (status = 404, description = "Candidate unavailable"), (status = 500, description = "Cannot read schools or logo directory"), (status = 503, description = "Image workers busy")
))]
pub(crate) async fn list_candidate_organizations(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match candidate_schools(&state, &id).await {
        Ok(schools) => observations(&state, schools).await,
        Err(error) => error.into_response(),
    }
}

async fn observations(state: &AppState, schools: Vec<OrganizationDetails>) -> Response {
    let response = match state.contest().observe_logos(schools).await {
        Ok(rows) => Json(
            rows.into_iter()
                .map(OrganizationLogoResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => ApiError::from_export(&error).into_response(),
    };
    no_store(response)
}

#[utoipa::path(get, path = "/api/v2/organizations/{organization_id}/logo", operation_id = "getOrganizationLogo", params(("organization_id" = String, Path), ("If-None-Match" = Option<String>, Header)), responses(
    (status = 200, description = "Current raster image; SVG is rendered to PNG", content((Download = "image/png"), (Download = "image/jpeg"), (Download = "image/webp"))),
    (status = 304, description = "Image content unchanged"), (status = 404, description = "School or unique logo unavailable"), (status = 500, description = "Logo read or decoding failed"), (status = 503, description = "Image workers busy")
))]
pub(crate) async fn organization_logo(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match state.contest().list_organizations().await {
        Ok(schools) => logo(&state, schools, &id, &headers, false).await,
        Err(error) => ApiError::from_contest(error).into_response(),
    }
}

#[utoipa::path(get, path = "/api/v2/imports/{import_id}/organizations/{organization_id}/logo", operation_id = "getCandidateOrganizationLogo", security(("sessionCookie" = [])), params(("import_id" = String, Path), ("organization_id" = String, Path)), responses(
    (status = 200, description = "Pending school's raster image; SVG is rendered to PNG", content((Download = "image/png"), (Download = "image/jpeg"), (Download = "image/webp"))),
    (status = 401, description = "Authentication required"), (status = 403, description = "Administrator required"), (status = 404, description = "Candidate, school or unique logo unavailable"), (status = 500, description = "Logo read or decoding failed"), (status = 503, description = "Image workers busy")
))]
pub(crate) async fn candidate_logo(
    State(state): State<AppState>,
    Path((import_id, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let response = match candidate_schools(&state, &import_id).await {
        Ok(schools) => logo(&state, schools, &id, &headers, true).await,
        Err(error) => error.into_response(),
    };
    no_store(response)
}

async fn candidate_schools(
    state: &AppState,
    id: &str,
) -> Result<Vec<OrganizationDetails>, ApiError> {
    let candidate = state
        .import()
        .read_pending()
        .await
        .map_err(ApiError::from_import)?
        .filter(|candidate| candidate.candidate_id().to_string() == id)
        .ok_or_else(|| ApiError::not_found("import_candidate_unavailable"))?;
    Ok(candidate.diff().organizations.clone())
}

async fn logo(
    state: &AppState,
    schools: Vec<OrganizationDetails>,
    id: &str,
    headers: &HeaderMap,
    private: bool,
) -> Response {
    let Some(school) = schools
        .into_iter()
        .find(|school| format!("INST-{:03}", school.organization_id) == id)
    else {
        return no_store(ApiError::not_found("organization_not_found").into_response());
    };
    match state.contest().organization_logo(school).await {
        Ok(image) => {
            let etag = format!("\"{}\"", hex::encode(Sha256::digest(&image.bytes)));
            let unchanged = !private
                && headers
                    .get_all(header::IF_NONE_MATCH)
                    .iter()
                    .filter_map(|value| value.to_str().ok())
                    .flat_map(|value| value.split(','))
                    .any(|value| {
                        let value = value.trim();
                        value == "*" || value.strip_prefix("W/").unwrap_or(value) == etag
                    });
            let mut response = if unchanged {
                StatusCode::NOT_MODIFIED.into_response()
            } else {
                ([(header::CONTENT_TYPE, image.content_type)], image.bytes).into_response()
            };
            if let Ok(value) = etag.parse() {
                response.headers_mut().insert(header::ETAG, value);
            }
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("public, no-cache"),
            );
            response.headers_mut().insert(
                header::X_CONTENT_TYPE_OPTIONS,
                axum::http::HeaderValue::from_static("nosniff"),
            );
            response
        }
        Err(error) => no_store(ApiError::from_export(&error).into_response()),
    }
}

#[utoipa::path(get, path = "/api/v2/exports/domjudge", operation_id = "exportDomjudge", security(("sessionCookie" = [])), responses(
    (status = 200, description = "Complete committed roster, current passwords and PNG logos; never cache", body = Download, content_type = "application/zip"),
    (status = 401, description = "Authentication required"), (status = 403, description = "Administrator required"), (status = 409, description = "Import a complete roster first"), (status = 500, description = "Export failed; no partial archive"), (status = 503, description = "Export worker busy")
))]
pub(crate) async fn export_domjudge(State(state): State<AppState>) -> Response {
    no_store(match state.contest().export_domjudge().await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "application/zip"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"domjudge-export.zip\"",
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(error) => ApiError::from_export(&error).into_response(),
    })
}
fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
