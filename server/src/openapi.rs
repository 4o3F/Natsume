use axum::{Json, Router, extract::Path, http::StatusCode, routing::get};
use serde::Serialize;
use utoipa::{OpenApi, ToSchema, openapi::OpenApi as OpenApiDocument};
use uuid::Uuid;

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    status: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct EnrollmentApprovalAccepted {
    enrollment_request_id: Uuid,
    certificate_scope: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct OpenApiProblemDetails {
    #[serde(rename = "type")]
    type_uri: String,
    title: String,
    status: u16,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    correlation_id: Uuid,
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Natsume V2",
        version = "2.5.0",
        description = "Rust-owned Phase 0 API contract. Version 2.5.0 is the retained API skeleton baseline and is independent of the 2.0.0 package version. The Phase 0 blueprint router exposes only GET /api/v2/health; imports, enrollment-request approval and explicit device-action paths are frozen compatibility contracts, not currently mounted runtime routes. Catalogue routes in the architecture do not replace them implicitly."
    ),
    paths(
        get_health,
        create_csv_import,
        commit_csv_import,
        approve_enrollment,
        sync_device_state,
        sync_device_secret
    ),
    components(schemas(HealthResponse, EnrollmentApprovalAccepted, OpenApiProblemDetails))
)]
struct ApiDocument;

#[utoipa::path(
    get,
    path = "/api/v2/health",
    operation_id = "getHealth",
    responses((status = 200, description = "Server process is healthy", body = HealthResponse))
)]
async fn get_health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_owned(),
    })
}

#[utoipa::path(
    post,
    path = "/api/v2/imports",
    operation_id = "createCsvImport",
    summary = "Upload one authoritative seat/account/password CSV",
    responses((status = 202, description = "Import staged for masked preview"))
)]
#[allow(dead_code)]
fn create_csv_import() -> StatusCode {
    StatusCode::ACCEPTED
}

#[utoipa::path(
    post,
    path = "/api/v2/imports/{import_id}:commit",
    operation_id = "commitCsvImport",
    params(("import_id" = Uuid, Path, description = "CSV import identifier")),
    responses((status = 200, description = "Atomic domain commit; no device sync is implied"))
)]
#[allow(dead_code)]
fn commit_csv_import(Path(_import_id): Path<Uuid>) -> StatusCode {
    StatusCode::OK
}

#[utoipa::path(
    post,
    path = "/api/v2/enrollment-requests/{request_id}:approve",
    operation_id = "approveEnrollment",
    summary = "Approve Device Enrollment and issue only the QUIC client certificate",
    params(("request_id" = Uuid, Path, description = "Enrollment request identifier")),
    responses((
        status = 202,
        description = "Device Identity certificate issuance workflow accepted; no Gateway credential is issued",
        body = EnrollmentApprovalAccepted
    ))
)]
#[allow(dead_code)]
fn approve_enrollment(
    Path(request_id): Path<Uuid>,
) -> (StatusCode, Json<EnrollmentApprovalAccepted>) {
    (
        StatusCode::ACCEPTED,
        Json(EnrollmentApprovalAccepted {
            enrollment_request_id: request_id,
            certificate_scope: "device_identity".to_owned(),
        }),
    )
}

#[utoipa::path(
    post,
    path = "/api/v2/devices/{device_id}/actions/sync-state",
    operation_id = "syncDeviceState",
    description = "Creates an explicit non-secret state Command. If required, the Device requests its Gateway certificate over the authenticated QUIC session as a command-bound subflow.",
    params(("device_id" = String, Path, description = "Device identifier")),
    responses((
        status = 202,
        description = "Explicit SYNC_STATE operation created; Gateway certificate issuance may occur within the command"
    ))
)]
#[allow(dead_code)]
fn sync_device_state(Path(_device_id): Path<String>) -> StatusCode {
    StatusCode::ACCEPTED
}

#[utoipa::path(
    post,
    path = "/api/v2/devices/{device_id}/actions/sync-secret",
    operation_id = "syncDeviceSecret",
    description = "Requires a human operator, re-authentication and an audit reason.",
    params(("device_id" = String, Path, description = "Device identifier")),
    responses((status = 202, description = "Explicit human-triggered SYNC_SECRET operation created"))
)]
#[allow(dead_code)]
fn sync_device_secret(Path(_device_id): Path<String>) -> StatusCode {
    StatusCode::ACCEPTED
}

/// Returns the Axum router fragment for paths with an unambiguous matcher.
///
/// The current blueprint binary does not mount this router; the Server HTTP runtime owns
/// that later integration.
pub fn router() -> Router {
    // Axum 0.8 cannot route action-style `{id}:verb` segments. Phase 0 keeps those
    // accepted compatibility paths in the Rust-owned document until their runtime
    // implementation freezes a compatible matcher.
    Router::new().route("/api/v2/health", get(get_health))
}

/// Builds the server-owned `OpenAPI` document without reading a committed snapshot.
#[must_use]
pub fn openapi() -> OpenApiDocument {
    ApiDocument::openapi()
}
