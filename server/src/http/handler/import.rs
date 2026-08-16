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

use crate::{
    application::{
        import::{
            self, ImportBindingImpact, ImportError, ImportMappingChange, PendingImportCandidate,
            PreviewToken, RedactedImportPreview,
        },
        operator::{self, OperatorIdentity},
    },
    audit::CorrelationId,
};

use super::super::{AppState, error::ApiError, middleware, not_found};

pub(crate) const CSV_IMPORT_BODY_LIMIT_BYTES: usize = 4_194_304;
pub(crate) const IMPORT_COMMIT_BODY_LIMIT_BYTES: usize = 4_096;

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
    if let Err(error) = operator::require_admin(identity.role()) {
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
    /// RFC 3339 UTC timestamp with a trailing Z.
    #[schema(value_type = String, format = DateTime)]
    expires_at: String,
    baseline_configuration_revision: i64,
    baseline_binding_revision: i64,
    diff: ImportRedactedDiff,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportPendingSummary {
    candidate_id: Uuid,
    /// RFC 3339 UTC timestamp with a trailing Z.
    #[schema(value_type = String, format = DateTime)]
    expires_at: String,
    baseline_configuration_revision: i64,
    baseline_binding_revision: i64,
    diff: ImportRedactedDiff,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportPendingResponse {
    #[schema(required = true)]
    pending: Option<ImportPendingSummary>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportCommitRequest {
    #[schema(
        write_only,
        min_length = 43,
        max_length = 43,
        pattern = "^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$"
    )]
    preview_token: String,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportCommitResponse {
    configuration_revision: i64,
    binding_revision: i64,
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
    match import::create_import_candidate(
        &state.database,
        &state.vault_master_key_path,
        &body,
        correlation_id,
    )
    .await
    {
        Ok(created) => {
            let response = ImportPreviewResponse {
                candidate_id: created.candidate_id(),
                preview_token: encode_preview_token(created.preview_token().as_bytes()),
                expires_at: created.expires_at().to_owned(),
                baseline_configuration_revision: created.baseline_configuration_revision(),
                baseline_binding_revision: created.baseline_binding_revision(),
                diff: ImportRedactedDiff::from(created.diff()),
            };
            json_response(StatusCode::CREATED, &response, correlation_id)
        }
        Err(error @ (ImportError::InvalidCsv(_) | ImportError::CandidateInvalid)) => {
            match import::audit_invalid_import_upload(&state.database, correlation_id).await {
                Ok(()) => ApiError::from_import(error, correlation_id).into_response(),
                Err(audit_error) => {
                    ApiError::from_import(audit_error, correlation_id).into_response()
                }
            }
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
    match import::read_pending_import_candidate(&state.database, correlation_id).await {
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
        (status = 200, description = "CSV import committed", body = ImportCommitResponse),
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
    match import::commit_import(
        &state.database,
        &state.vault_master_key_path,
        import_id,
        &PreviewToken::from_bytes(token),
        correlation_id,
    )
    .await
    {
        Ok(committed) => json_response(
            StatusCode::OK,
            &ImportCommitResponse {
                configuration_revision: committed.configuration_revision(),
                binding_revision: committed.binding_revision(),
            },
            correlation_id,
        ),
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
    match import::discard_import(&state.database, import_id, correlation_id).await {
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
            expires_at: pending.expires_at().to_owned(),
            baseline_configuration_revision: pending.baseline_configuration_revision(),
            baseline_binding_revision: pending.baseline_binding_revision(),
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

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    use axum::{
        Router,
        body::Body,
        http::{HeaderValue, Method, Request, StatusCode, header},
    };
    use diesel::{QueryableByName, RunQueryDsl, sql_types::Text};
    use serde::Deserialize;
    use serde_json::Value;
    use snafu::Snafu;
    use uuid::Uuid;

    use crate::{
        application::operator::{OperatorRole, sign_in, tests::PasswordVerificationTestGuard},
        audit::CorrelationId,
        db::{
            Database,
            tests::{test_data_version, test_observer},
        },
        vault::ensure_master_key,
    };

    use super::super::super::{
        router,
        tests::{
            Captured, SupportFailure, TestDatabase, canonical_correlation_id, check_error_response,
            drive, header_text, normalized_error_response_body, response_body_text, seed_operator,
            unused_web_root,
        },
    };
    use super::{
        CSV_IMPORT_BODY_LIMIT_BYTES, IMPORT_COMMIT_BODY_LIMIT_BYTES, encode_preview_token,
    };

    const ADMIN_LOGIN: &str = "import-http-admin";
    const VIEWER_LOGIN: &str = "import-http-viewer";
    const PASSWORD: &str = "import-http-operator-password-canary";
    const CSV_PASSWORD_A: &str = "import-http-csv-password-canary-a";
    const CSV_PASSWORD_B: &str = "import-http-csv-password-canary-b";
    const IMPORT_ID: &str = "01900000-0000-7000-8000-000000000401";

    #[tokio::test]
    async fn all_import_operations_enforce_authentication_and_admin_role() -> Result<(), TestFailure>
    {
        let fixture = ImportHttpFixture::new().await?;
        fixture
            .seed_operator(VIEWER_LOGIN, OperatorRole::Viewer)
            .await?;
        let viewer_cookie = fixture.session_cookie(VIEWER_LOGIN).await?;
        let application = fixture.router();

        for operation in ImportOperation::ALL {
            let unauthenticated = drive(&application, operation.request(None)?).await?;
            check_error_response(
                &unauthenticated,
                StatusCode::UNAUTHORIZED,
                "Unauthorized",
                "AUTHENTICATION_FAILED",
            )?;
            let viewer = drive(
                &application,
                operation.request(Some(viewer_cookie.as_str()))?,
            )
            .await?;
            check_error_response(
                &viewer,
                StatusCode::FORBIDDEN,
                "Forbidden",
                "AUTHORIZATION_DENIED",
            )?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn pending_read_is_null_and_read_only_when_no_candidate_exists() -> Result<(), TestFailure>
    {
        let fixture = ImportHttpFixture::new().await?;
        fixture
            .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
            .await?;
        let admin_cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
        let application = fixture.router();
        let mut observer = test_observer(&fixture.database.path)
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let before = import_snapshot(&fixture.database.database).await?;
        let data_version_before =
            test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;

        let response = read_pending(&application, &admin_cookie).await?;
        assert_pending_null(&response)?;

        let after = import_snapshot(&fixture.database.database).await?;
        let data_version_after =
            test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        if before != after || data_version_before != data_version_after {
            return Err(TestFailure::RejectedBoundaryWroteData);
        }
        Ok(())
    }

    #[tokio::test]
    async fn pending_read_replays_the_exact_redacted_summary_without_the_token()
    -> Result<(), TestFailure> {
        let fixture = ImportHttpFixture::new().await?;
        fixture
            .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
            .await?;
        let admin_cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
        let application = fixture.router();
        let created = upload(
            &application,
            &admin_cookie,
            &format!(
                "seat,account,password\nB-02,team-b,{CSV_PASSWORD_B}\nA-01,team-a,{CSV_PASSWORD_A}"
            ),
        )
        .await?;
        let preview = assert_preview(&created)?;

        let response = read_pending(&application, &admin_cookie).await?;
        let pending = assert_pending_summary(&response)?;
        if pending.candidate_id != preview.candidate_id
            || pending.expires_at != preview.expires_at
            || pending.baseline_configuration_revision != preview.baseline_configuration_revision
            || pending.baseline_binding_revision != preview.baseline_binding_revision
            || serde_json::to_vec(&pending.diff).map_err(|_| TestFailure::ResponseJsonInvalid)?
                != serde_json::to_vec(&preview.diff)
                    .map_err(|_| TestFailure::ResponseJsonInvalid)?
        {
            return Err(TestFailure::PendingResponseChanged);
        }
        let body = response_body_text(&response)?;
        if body.contains("preview_token") || body.contains(&preview.preview_token) {
            return Err(TestFailure::SecretEscaped);
        }
        Ok(())
    }

    #[tokio::test]
    async fn pending_read_tolerates_unknown_stored_preview_fields() -> Result<(), TestFailure> {
        let fixture = ImportHttpFixture::new().await?;
        fixture
            .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
            .await?;
        let admin_cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
        let application = fixture.router();
        let created = upload(
            &application,
            &admin_cookie,
            "seat,account,password\nA-01,team-a,forward-compatible-password-canary",
        )
        .await?;
        let preview = assert_preview(&created)?;
        add_unknown_pending_preview_field(&fixture.database.database, &preview.candidate_id)
            .await?;

        let response = read_pending(&application, &admin_cookie).await?;
        let pending = assert_pending_summary(&response)?;
        if pending.candidate_id != preview.candidate_id
            || serde_json::to_vec(&pending.diff).map_err(|_| TestFailure::ResponseJsonInvalid)?
                != serde_json::to_vec(&preview.diff)
                    .map_err(|_| TestFailure::ResponseJsonInvalid)?
        {
            return Err(TestFailure::PendingResponseChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn pending_read_lazily_expires_and_audits_an_expired_candidate() -> Result<(), TestFailure>
    {
        let fixture = ImportHttpFixture::new().await?;
        fixture
            .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
            .await?;
        let admin_cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
        let application = fixture.router();
        let created = upload(
            &application,
            &admin_cookie,
            "seat,account,password\nA-01,team-a,expiry-password-canary",
        )
        .await?;
        let preview = assert_preview(&created)?;
        age_pending_candidate(&fixture.database.database, &preview.candidate_id).await?;

        let response = read_pending(&application, &admin_cookie).await?;
        assert_pending_null(&response)?;
        let snapshot = import_snapshot(&fixture.database.database).await?;
        if snapshot.candidates != 0 || snapshot.vault != 0 || snapshot.audits != 2 {
            return Err(TestFailure::PendingResponseChanged);
        }
        assert_expiry_audit(&fixture.database.database, &preview.candidate_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_pending_preview_is_a_read_only_internal_failure() -> Result<(), TestFailure> {
        let fixture = ImportHttpFixture::new().await?;
        fixture
            .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
            .await?;
        let admin_cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
        let application = fixture.router();
        let created = upload(
            &application,
            &admin_cookie,
            "seat,account,password\nA-01,team-a,corrupt-preview-password-canary",
        )
        .await?;
        let preview = assert_preview(&created)?;
        corrupt_pending_preview(&fixture.database.database, &preview.candidate_id).await?;
        let mut observer = test_observer(&fixture.database.path)
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let before = import_snapshot(&fixture.database.database).await?;
        let data_version_before =
            test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;

        let response = read_pending(&application, &admin_cookie).await?;
        check_error_response(
            &response,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            "INTERNAL_ERROR",
        )?;
        let after = import_snapshot(&fixture.database.database).await?;
        let data_version_after =
            test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        if before != after || data_version_before != data_version_after {
            return Err(TestFailure::RejectedBoundaryWroteData);
        }
        if response_body_text(&response)?.contains(&preview.preview_token) {
            return Err(TestFailure::SecretEscaped);
        }
        Ok(())
    }

    #[tokio::test]
    async fn upload_and_commit_limits_precede_their_inner_boundaries() -> Result<(), TestFailure> {
        let fixture = ImportHttpFixture::new().await?;
        fixture
            .seed_operator(VIEWER_LOGIN, OperatorRole::Viewer)
            .await?;
        fixture
            .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
            .await?;
        let viewer_cookie = fixture.session_cookie(VIEWER_LOGIN).await?;
        let admin_cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
        let application = fixture.router();
        let before = import_snapshot(&fixture.database.database).await?;

        let unauthenticated = drive(&application, oversized_upload_request(None)?).await?;
        assert_transport_payload_too_large(&unauthenticated)?;

        let viewer_wrong_media = drive(
            &application,
            request_with_cookie(
                Method::POST,
                "/api/v2/imports",
                Some(&viewer_cookie),
                Some("application/json"),
                Body::from("viewer-media-type-canary"),
            )?,
        )
        .await?;
        check_error_response(
            &viewer_wrong_media,
            StatusCode::FORBIDDEN,
            "Forbidden",
            "AUTHORIZATION_DENIED",
        )?;

        let oversized_commit =
            drive(&application, oversized_commit_request(&admin_cookie)?).await?;
        assert_transport_payload_too_large(&oversized_commit)?;
        if import_snapshot(&fixture.database.database).await? != before {
            return Err(TestFailure::RejectedBoundaryWroteData);
        }
        Ok(())
    }

    #[tokio::test]
    async fn unknown_candidate_and_wrong_token_are_publicly_indistinguishable()
    -> Result<(), TestFailure> {
        let fixture = ImportHttpFixture::new().await?;
        fixture
            .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
            .await?;
        let admin_cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
        let application = fixture.router();
        let preview = upload(
            &application,
            &admin_cookie,
            "seat,account,password\nA-01,team-a,oracle-password-canary",
        )
        .await?;
        let preview = assert_preview(&preview)?;
        let wrong_token = encode_preview_token(&[0x55; 32]);
        let known = commit(
            &application,
            &admin_cookie,
            &preview.candidate_id,
            &wrong_token,
        )
        .await?;
        let unknown = commit(
            &application,
            &admin_cookie,
            "01900000-0000-7000-8000-000000000402",
            &wrong_token,
        )
        .await?;
        for response in [&known, &unknown] {
            check_error_response(
                response,
                StatusCode::NOT_FOUND,
                "Not Found",
                "IMPORT_CANDIDATE_UNAVAILABLE",
            )?;
        }
        if known.status != unknown.status
            || normalized_error_response_body(&known)? != normalized_error_response_body(&unknown)?
        {
            return Err(TestFailure::CandidateUnavailableOracleChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn upload_commit_stale_repreview_commit_and_discard_flow_is_exact()
    -> Result<(), TestFailure> {
        let fixture = ImportHttpFixture::new().await?;
        fixture
            .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
            .await?;
        let admin_cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
        let application = fixture.router();
        let csv = format!(
            "seat,account,password\nA-01,team-a,{CSV_PASSWORD_A}\nB-02,team-b,{CSV_PASSWORD_B}"
        );

        let first = upload(&application, &admin_cookie, &csv).await?;
        let first_preview = assert_preview(&first)?;
        if first_preview.baseline_configuration_revision != 0
            || first_preview.baseline_binding_revision != 0
        {
            return Err(TestFailure::PreviewResponseChanged);
        }
        let pending = upload(&application, &admin_cookie, &csv).await?;
        check_error_response(
            &pending,
            StatusCode::CONFLICT,
            "Conflict",
            "IMPORT_CANDIDATE_PENDING",
        )?;

        let wrong_token = encode_preview_token(&[0x55; 32]);
        let mismatch = commit(
            &application,
            &admin_cookie,
            &first_preview.candidate_id,
            &wrong_token,
        )
        .await?;
        check_error_response(
            &mismatch,
            StatusCode::NOT_FOUND,
            "Not Found",
            "IMPORT_CANDIDATE_UNAVAILABLE",
        )?;
        bump_configuration_revision(&fixture.database.database).await?;
        let stale = commit(
            &application,
            &admin_cookie,
            &first_preview.candidate_id,
            &first_preview.preview_token,
        )
        .await?;
        check_error_response(
            &stale,
            StatusCode::CONFLICT,
            "Conflict",
            "IMPORT_PREVIEW_STALE",
        )?;
        assert_secret_absent(&[&pending, &mismatch, &stale], &first_preview.preview_token)?;

        let discarded_stale =
            discard(&application, &admin_cookie, &first_preview.candidate_id).await?;
        assert_empty_success(&discarded_stale, StatusCode::NO_CONTENT)?;
        let fresh = upload(&application, &admin_cookie, &csv).await?;
        let fresh_preview = assert_preview(&fresh)?;
        if fresh_preview.baseline_configuration_revision != 1
            || fresh_preview.baseline_binding_revision != 0
        {
            return Err(TestFailure::PreviewResponseChanged);
        }
        let committed = commit(
            &application,
            &admin_cookie,
            &fresh_preview.candidate_id,
            &fresh_preview.preview_token,
        )
        .await?;
        assert_commit_success(&committed, 2, 0)?;

        let discard_preview = upload(&application, &admin_cookie, &csv).await?;
        let discard_preview = assert_preview(&discard_preview)?;
        if discard_preview.baseline_configuration_revision != 2
            || discard_preview.baseline_binding_revision != 0
        {
            return Err(TestFailure::PreviewResponseChanged);
        }
        let discarded = discard(&application, &admin_cookie, &discard_preview.candidate_id).await?;
        assert_empty_success(&discarded, StatusCode::NO_CONTENT)?;
        let repeated = discard(&application, &admin_cookie, &discard_preview.candidate_id).await?;
        check_error_response(
            &repeated,
            StatusCode::NOT_FOUND,
            "Not Found",
            "IMPORT_CANDIDATE_UNAVAILABLE",
        )?;

        assert_passwords_absent(&[
            &first, &pending, &mismatch, &stale, &fresh, &committed, &discarded, &repeated,
        ])?;
        let audit_json = import_audit_json(&fixture.database.database).await?;
        if audit_json.contains(CSV_PASSWORD_A)
            || audit_json.contains(CSV_PASSWORD_B)
            || audit_json.contains(&first_preview.preview_token)
            || audit_json.contains(&fresh_preview.preview_token)
        {
            return Err(TestFailure::SecretEscaped);
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejected_upload_limits_media_and_ids_preserve_audit_and_zero_write_boundaries()
    -> Result<(), TestFailure> {
        let fixture = ImportHttpFixture::new().await?;
        fixture
            .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
            .await?;
        let admin_cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
        let application = fixture.router();
        let invalid_password = "rejected-upload-password-canary";
        let invalid_csv =
            format!("seat,account,password\nA-01,team-a,{invalid_password},unexpected-column");
        let invalid = upload(&application, &admin_cookie, &invalid_csv).await?;
        check_error_response(
            &invalid,
            StatusCode::BAD_REQUEST,
            "Bad Request",
            "IMPORT_CANDIDATE_INVALID",
        )?;
        assert_invalid_upload_audit(&fixture.database.database).await?;

        let mut observer = test_observer(&fixture.database.path)
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let before = import_snapshot(&fixture.database.database).await?;
        let data_version_before =
            test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let oversized = oversized_upload_request(Some(&admin_cookie))?;
        let oversized = drive(&application, oversized).await?;
        assert_transport_payload_too_large(&oversized)?;

        let wrong_media = drive(
            &application,
            request_with_cookie(
                Method::POST,
                "/api/v2/imports",
                Some(&admin_cookie),
                Some("application/json"),
                Body::from(invalid_csv),
            )?,
        )
        .await?;
        check_error_response(
            &wrong_media,
            StatusCode::BAD_REQUEST,
            "Bad Request",
            "INVALID_REQUEST",
        )?;

        let valid_token = encode_preview_token(&[0x66; 32]);
        let invalid_commit = commit(
            &application,
            &admin_cookie,
            "NOT-CANONICAL-IMPORT-ID-CANARY",
            &valid_token,
        )
        .await?;
        check_error_response(
            &invalid_commit,
            StatusCode::BAD_REQUEST,
            "Bad Request",
            "INVALID_REQUEST",
        )?;
        let invalid_discard = discard(
            &application,
            &admin_cookie,
            "NOT-CANONICAL-IMPORT-ID-CANARY",
        )
        .await?;
        check_error_response(
            &invalid_discard,
            StatusCode::BAD_REQUEST,
            "Bad Request",
            "INVALID_REQUEST",
        )?;
        let after = import_snapshot(&fixture.database.database).await?;
        let data_version_after =
            test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        if before != after || data_version_before != data_version_after {
            return Err(TestFailure::RejectedBoundaryWroteData);
        }
        for response in [
            &invalid,
            &oversized,
            &wrong_media,
            &invalid_commit,
            &invalid_discard,
        ] {
            if response_body_text(response)?.contains(invalid_password)
                || response_body_text(response)?.contains(&valid_token)
            {
                return Err(TestFailure::SecretEscaped);
            }
        }
        Ok(())
    }

    #[derive(Clone, Copy)]
    enum ImportOperation {
        Read,
        Upload,
        Commit,
        Discard,
    }

    impl ImportOperation {
        const ALL: [Self; 4] = [Self::Read, Self::Upload, Self::Commit, Self::Discard];

        fn request(self, cookie: Option<&str>) -> Result<Request<Body>, TestFailure> {
            match self {
                Self::Read => {
                    request_with_cookie(Method::GET, "/api/v2/imports", cookie, None, Body::empty())
                }
                Self::Upload => request_with_cookie(
                    Method::POST,
                    "/api/v2/imports",
                    cookie,
                    Some("text/csv; charset=utf-8"),
                    Body::from("seat,account,password\nA-01,team-a,role-password"),
                ),
                Self::Commit => request_with_cookie(
                    Method::POST,
                    &format!("/api/v2/imports/{IMPORT_ID}/actions/commit"),
                    cookie,
                    Some("application/json"),
                    Body::from(format!(
                        "{{\"preview_token\":\"{}\"}}",
                        encode_preview_token(&[0x44; 32])
                    )),
                ),
                Self::Discard => request_with_cookie(
                    Method::POST,
                    &format!("/api/v2/imports/{IMPORT_ID}/actions/discard"),
                    cookie,
                    None,
                    Body::empty(),
                ),
            }
        }
    }

    struct ImportHttpFixture {
        database: TestDatabase,
        key_directory: PathBuf,
        master_key_path: PathBuf,
    }

    impl ImportHttpFixture {
        async fn new() -> Result<Self, TestFailure> {
            let database = TestDatabase::new().await?;
            let key_directory = std::env::temp_dir()
                .join(format!("natsume-import-http-vault-test-{}", Uuid::now_v7()));
            fs::create_dir(&key_directory).map_err(|_| TestFailure::FixtureFailed)?;
            fs::set_permissions(&key_directory, fs::Permissions::from_mode(0o700))
                .map_err(|_| TestFailure::FixtureFailed)?;
            let master_key_path = key_directory.join("master.key");
            ensure_master_key(&master_key_path).map_err(|_| TestFailure::FixtureFailed)?;
            Ok(Self {
                database,
                key_directory,
                master_key_path,
            })
        }

        fn router(&self) -> Router {
            router(
                self.database.database.clone(),
                &self.master_key_path,
                unused_web_root(),
            )
        }

        async fn seed_operator(
            &self,
            login_name: &str,
            role: OperatorRole,
        ) -> Result<(), TestFailure> {
            seed_operator(&self.database.database, login_name, role, PASSWORD).await?;
            Ok(())
        }

        async fn session_cookie(&self, login_name: &str) -> Result<String, TestFailure> {
            let _verification_guard = PasswordVerificationTestGuard::acquire().await;
            let session = sign_in(
                &self.database.database,
                CorrelationId::from_uuid(Uuid::now_v7()),
                login_name,
                PASSWORD.to_owned(),
            )
            .await
            .map_err(|_| TestFailure::FixtureFailed)?;
            Ok(format!(
                "__Secure-natsume_session={}",
                session.credential().to_wire().expose()
            ))
        }
    }

    impl Drop for ImportHttpFixture {
        fn drop(&mut self) {
            let _cleanup_result = fs::remove_dir_all(&self.key_directory);
        }
    }

    async fn upload(
        application: &Router,
        cookie: &str,
        csv: &str,
    ) -> Result<Captured, TestFailure> {
        let request = request_with_cookie(
            Method::POST,
            "/api/v2/imports",
            Some(cookie),
            Some("text/csv; charset=utf-8"),
            Body::from(csv.to_owned()),
        )?;
        drive(application, request).await.map_err(TestFailure::from)
    }

    async fn read_pending(application: &Router, cookie: &str) -> Result<Captured, TestFailure> {
        let request = request_with_cookie(
            Method::GET,
            "/api/v2/imports",
            Some(cookie),
            None,
            Body::empty(),
        )?;
        drive(application, request).await.map_err(TestFailure::from)
    }

    async fn commit(
        application: &Router,
        cookie: &str,
        import_id: &str,
        preview_token: &str,
    ) -> Result<Captured, TestFailure> {
        let request = request_with_cookie(
            Method::POST,
            &format!("/api/v2/imports/{import_id}/actions/commit"),
            Some(cookie),
            Some("application/json"),
            Body::from(serde_json::json!({"preview_token": preview_token}).to_string()),
        )?;
        drive(application, request).await.map_err(TestFailure::from)
    }

    async fn discard(
        application: &Router,
        cookie: &str,
        import_id: &str,
    ) -> Result<Captured, TestFailure> {
        let request = request_with_cookie(
            Method::POST,
            &format!("/api/v2/imports/{import_id}/actions/discard"),
            Some(cookie),
            None,
            Body::empty(),
        )?;
        drive(application, request).await.map_err(TestFailure::from)
    }

    fn request_with_cookie(
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        content_type: Option<&str>,
        body: Body,
    ) -> Result<Request<Body>, TestFailure> {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        request
            .body(body)
            .map_err(|_| TestFailure::RequestBuildFailed)
    }

    fn oversized_upload_request(cookie: Option<&str>) -> Result<Request<Body>, TestFailure> {
        let mut request = request_with_cookie(
            Method::POST,
            "/api/v2/imports",
            cookie,
            Some("text/csv"),
            Body::from(vec![b'!'; CSV_IMPORT_BODY_LIMIT_BYTES + 1]),
        )?;
        request.headers_mut().insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&(CSV_IMPORT_BODY_LIMIT_BYTES + 1).to_string())
                .map_err(|_| TestFailure::RequestBuildFailed)?,
        );
        Ok(request)
    }

    fn oversized_commit_request(cookie: &str) -> Result<Request<Body>, TestFailure> {
        let mut request = request_with_cookie(
            Method::POST,
            &format!("/api/v2/imports/{IMPORT_ID}/actions/commit"),
            Some(cookie),
            Some("application/json"),
            Body::from(vec![b'!'; IMPORT_COMMIT_BODY_LIMIT_BYTES + 1]),
        )?;
        request.headers_mut().insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&(IMPORT_COMMIT_BODY_LIMIT_BYTES + 1).to_string())
                .map_err(|_| TestFailure::RequestBuildFailed)?,
        );
        Ok(request)
    }

    fn assert_preview(response: &Captured) -> Result<PreviewEvidence, TestFailure> {
        if response.status != StatusCode::CREATED {
            return Err(TestFailure::PreviewResponseChanged);
        }
        canonical_correlation_id(&response.headers)?;
        let preview: PreviewEvidence =
            serde_json::from_slice(&response.body).map_err(|_| TestFailure::ResponseJsonInvalid)?;
        let candidate_id = Uuid::parse_str(&preview.candidate_id)
            .map_err(|_| TestFailure::PreviewResponseChanged)?;
        if candidate_id.get_version_num() != 7
            || candidate_id.to_string() != preview.candidate_id
            || preview.preview_token.len() != 43
            || !preview.expires_at.ends_with('Z')
            || !is_rfc3339_utc(&preview.expires_at)
            || preview.diff.get("seats_added").is_none()
        {
            return Err(TestFailure::PreviewResponseChanged);
        }
        Ok(preview)
    }

    fn assert_pending_null(response: &Captured) -> Result<(), TestFailure> {
        if response.status != StatusCode::OK {
            return Err(TestFailure::PendingResponseChanged);
        }
        canonical_correlation_id(&response.headers)?;
        let value: PendingResponseEvidence =
            serde_json::from_slice(&response.body).map_err(|_| TestFailure::ResponseJsonInvalid)?;
        if value.pending.is_some() || response.body.as_slice() != br#"{"pending":null}"# {
            return Err(TestFailure::PendingResponseChanged);
        }
        Ok(())
    }

    fn assert_pending_summary(response: &Captured) -> Result<PendingSummaryEvidence, TestFailure> {
        if response.status != StatusCode::OK {
            return Err(TestFailure::PendingResponseChanged);
        }
        canonical_correlation_id(&response.headers)?;
        let value: PendingResponseEvidence =
            serde_json::from_slice(&response.body).map_err(|_| TestFailure::ResponseJsonInvalid)?;
        value.pending.ok_or(TestFailure::PendingResponseChanged)
    }

    fn is_rfc3339_utc(value: &str) -> bool {
        let bytes = value.as_bytes();
        if bytes.len() < 20
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || bytes[10] != b'T'
            || bytes[13] != b':'
            || bytes[16] != b':'
            || bytes.last() != Some(&b'Z')
        {
            return false;
        }
        let fractional = &bytes[19..bytes.len() - 1];
        if !fractional.is_empty()
            && (fractional[0] != b'.'
                || fractional.len() == 1
                || !fractional[1..].iter().all(u8::is_ascii_digit))
        {
            return false;
        }
        let parse = |start, end| {
            value
                .get(start..end)
                .and_then(|part| part.parse::<u32>().ok())
        };
        let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
            parse(0, 4),
            parse(5, 7),
            parse(8, 10),
            parse(11, 13),
            parse(14, 16),
            parse(17, 19),
        ) else {
            return false;
        };
        let leap_year =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let maximum_day = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap_year => 29,
            2 => 28,
            _ => return false,
        };
        (1..=maximum_day).contains(&day) && hour < 24 && minute < 60 && second < 60
    }

    fn assert_transport_payload_too_large(response: &Captured) -> Result<(), TestFailure> {
        let content_type = header_text(&response.headers, &header::CONTENT_TYPE)?;
        if response.status != StatusCode::PAYLOAD_TOO_LARGE
            || content_type
                .to_ascii_lowercase()
                .starts_with("application/json")
        {
            return Err(TestFailure::BodyLimitChanged);
        }
        canonical_correlation_id(&response.headers)?;
        Ok(())
    }

    fn assert_commit_success(
        response: &Captured,
        configuration_revision: i64,
        binding_revision: i64,
    ) -> Result<(), TestFailure> {
        if response.status != StatusCode::OK {
            return Err(TestFailure::CommitResponseChanged);
        }
        canonical_correlation_id(&response.headers)?;
        let value: Value =
            serde_json::from_slice(&response.body).map_err(|_| TestFailure::ResponseJsonInvalid)?;
        if value
            != serde_json::json!({
                "configuration_revision": configuration_revision,
                "binding_revision": binding_revision
            })
        {
            return Err(TestFailure::CommitResponseChanged);
        }
        Ok(())
    }

    fn assert_empty_success(
        response: &Captured,
        expected_status: StatusCode,
    ) -> Result<(), TestFailure> {
        if response.status != expected_status || !response.body.is_empty() {
            return Err(TestFailure::DiscardResponseChanged);
        }
        canonical_correlation_id(&response.headers)?;
        Ok(())
    }

    fn assert_passwords_absent(responses: &[&Captured]) -> Result<(), TestFailure> {
        for response in responses {
            let body = response_body_text(response)?;
            if body.contains(CSV_PASSWORD_A) || body.contains(CSV_PASSWORD_B) {
                return Err(TestFailure::SecretEscaped);
            }
        }
        Ok(())
    }

    fn assert_secret_absent(
        responses: &[&Captured],
        preview_token: &str,
    ) -> Result<(), TestFailure> {
        for response in responses {
            if response_body_text(response)?.contains(preview_token) {
                return Err(TestFailure::SecretEscaped);
            }
        }
        Ok(())
    }

    async fn bump_configuration_revision(database: &Database) -> Result<(), TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "UPDATE revision_counters SET configuration_revision = \
                     configuration_revision + 1 WHERE singleton = 1",
                )
                .execute(connection)
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map(|_| ())
            .map_err(|_| TestFailure::FixtureFailed)
    }

    async fn age_pending_candidate(
        database: &Database,
        candidate_id: &str,
    ) -> Result<(), TestFailure> {
        let candidate_id = candidate_id.to_owned();
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "UPDATE pending_import_candidate \
                     SET expires_at = '2000-01-01T00:00:00.000Z' \
                     WHERE candidate_id = ?",
                )
                .bind::<Text, _>(candidate_id)
                .execute(connection)
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map(|_| ())
            .map_err(|_| TestFailure::FixtureFailed)
    }

    async fn corrupt_pending_preview(
        database: &Database,
        candidate_id: &str,
    ) -> Result<(), TestFailure> {
        let candidate_id = candidate_id.to_owned();
        database
            .interact(move |connection| {
                diesel::sql_query("PRAGMA ignore_check_constraints = ON").execute(connection)?;
                let update = diesel::sql_query(
                    "UPDATE pending_import_candidate \
                     SET redacted_preview_json = '{not-json' \
                     WHERE candidate_id = ?",
                )
                .bind::<Text, _>(candidate_id)
                .execute(connection);
                let reset =
                    diesel::sql_query("PRAGMA ignore_check_constraints = OFF").execute(connection);
                update.and(reset)
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map(|_| ())
            .map_err(|_| TestFailure::FixtureFailed)
    }

    async fn add_unknown_pending_preview_field(
        database: &Database,
        candidate_id: &str,
    ) -> Result<(), TestFailure> {
        let candidate_id = candidate_id.to_owned();
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "UPDATE pending_import_candidate \
                     SET redacted_preview_json = json_set( \
                       redacted_preview_json, '$.future_preview_evidence', json('{\"version\":1}')) \
                     WHERE candidate_id = ?",
                )
                .bind::<Text, _>(candidate_id)
                .execute(connection)
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map(|_| ())
            .map_err(|_| TestFailure::FixtureFailed)
    }

    async fn assert_expiry_audit(
        database: &Database,
        candidate_id: &str,
    ) -> Result<(), TestFailure> {
        let candidate_id = candidate_id.to_owned();
        let audit = database
            .interact(move |connection| {
                diesel::sql_query(
                    "SELECT actor, action_kind, resource_type, resource_id, result, reason_code, \
                     redacted_detail_json, \
                     (SELECT COUNT(*) FROM audit_events \
                       WHERE action_kind = 'expire_import_candidate') AS count \
                     FROM audit_events \
                     WHERE action_kind = 'expire_import_candidate'",
                )
                .get_result::<InvalidAuditEvidence>(connection)
            })
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        if audit.count != 1
            || audit.actor != "system:expiry"
            || audit.action_kind != "expire_import_candidate"
            || audit.resource_type != "import_candidate"
            || audit.resource_id.as_deref() != Some(candidate_id.as_str())
            || audit.result != "succeeded"
            || audit.reason_code.as_deref() != Some("absolute_expiry_observed")
            || audit.redacted_detail_json != "{}"
        {
            return Err(TestFailure::ExpiryAuditChanged);
        }
        Ok(())
    }

    async fn assert_invalid_upload_audit(database: &Database) -> Result<(), TestFailure> {
        let audit = database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT actor, action_kind, resource_type, resource_id, result, reason_code, \
                     redacted_detail_json, \
                     (SELECT COUNT(*) FROM audit_events \
                       WHERE action_kind = 'create_import_candidate' AND result = 'rejected') \
                       AS count \
                     FROM audit_events \
                     WHERE action_kind = 'create_import_candidate' AND result = 'rejected'",
                )
                .get_result::<InvalidAuditEvidence>(connection)
            })
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        if audit.count != 1
            || audit.actor != "operator:self"
            || audit.action_kind != "create_import_candidate"
            || audit.resource_type != "import_candidate"
            || audit.resource_id.is_some()
            || audit.result != "rejected"
            || audit.reason_code.as_deref() != Some("candidate_invalid")
            || audit.redacted_detail_json != "{}"
        {
            return Err(TestFailure::InvalidAuditChanged);
        }
        Ok(())
    }

    async fn import_snapshot(database: &Database) -> Result<ImportSnapshot, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT \
                     (SELECT COUNT(*) FROM pending_import_candidate) AS candidates, \
                     (SELECT COUNT(*) FROM server_vault_records) AS vault, \
                     (SELECT COUNT(*) FROM audit_events \
                       WHERE action_kind LIKE '%import%') AS audits",
                )
                .get_result::<ImportSnapshot>(connection)
            })
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)
    }

    async fn import_audit_json(database: &Database) -> Result<String, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT COALESCE(group_concat(redacted_detail_json, ''), '') AS value \
                     FROM audit_events WHERE action_kind LIKE '%import%'",
                )
                .get_result::<TextEvidence>(connection)
            })
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
            .map(|row| row.value)
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)
    }

    #[derive(Deserialize)]
    struct PreviewEvidence {
        candidate_id: String,
        preview_token: String,
        expires_at: String,
        baseline_configuration_revision: i64,
        baseline_binding_revision: i64,
        diff: Value,
    }

    #[derive(Deserialize)]
    struct PendingResponseEvidence {
        pending: Option<PendingSummaryEvidence>,
    }

    #[derive(Deserialize)]
    struct PendingSummaryEvidence {
        candidate_id: String,
        expires_at: String,
        baseline_configuration_revision: i64,
        baseline_binding_revision: i64,
        diff: Value,
    }

    #[derive(QueryableByName)]
    struct InvalidAuditEvidence {
        #[diesel(sql_type = Text)]
        actor: String,
        #[diesel(sql_type = Text)]
        action_kind: String,
        #[diesel(sql_type = Text)]
        resource_type: String,
        #[diesel(sql_type = diesel::sql_types::Nullable<Text>)]
        resource_id: Option<String>,
        #[diesel(sql_type = Text)]
        result: String,
        #[diesel(sql_type = diesel::sql_types::Nullable<Text>)]
        reason_code: Option<String>,
        #[diesel(sql_type = Text)]
        redacted_detail_json: String,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        count: i64,
    }

    #[derive(Debug, PartialEq, Eq, QueryableByName)]
    struct ImportSnapshot {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        candidates: i64,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        vault: i64,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        audits: i64,
    }

    #[derive(QueryableByName)]
    struct TextEvidence {
        #[diesel(sql_type = Text)]
        value: String,
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("an HTTP test helper failed"))]
        #[snafu(context(false))]
        Support { source: SupportFailure },
        #[snafu(display("the import HTTP fixture failed"))]
        FixtureFailed,
        #[snafu(display("the import HTTP request could not be built"))]
        RequestBuildFailed,
        #[snafu(display("the import HTTP response was not valid JSON"))]
        ResponseJsonInvalid,
        #[snafu(display("the import preview response changed"))]
        PreviewResponseChanged,
        #[snafu(display("the pending import response changed"))]
        PendingResponseChanged,
        #[snafu(display("the import commit response changed"))]
        CommitResponseChanged,
        #[snafu(display("the import discard response changed"))]
        DiscardResponseChanged,
        #[snafu(display("the import body limit changed"))]
        BodyLimitChanged,
        #[snafu(display("the import candidate-unavailable oracle changed"))]
        CandidateUnavailableOracleChanged,
        #[snafu(display("import database evidence could not be read"))]
        DatabaseEvidenceFailed,
        #[snafu(display("the invalid-upload audit changed"))]
        InvalidAuditChanged,
        #[snafu(display("the expiry audit changed"))]
        ExpiryAuditChanged,
        #[snafu(display("a rejected import boundary wrote data"))]
        RejectedBoundaryWroteData,
        #[snafu(display("an import secret escaped the HTTP or audit boundary"))]
        SecretEscaped,
    }
}
