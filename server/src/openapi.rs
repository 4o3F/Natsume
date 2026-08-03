use axum::{Json, Router, extract::Path, http::StatusCode, routing::get};
use serde::{Deserialize, Serialize};
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

/// The only Panel-command kinds frozen by the Phase 0 HTTP contract.
///
/// Device-facing payloads remain bounded by this enum; arbitrary remote-management commands are
/// intentionally not representable.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum CommandKind {
    SyncState,
    SyncSecret,
}

/// No caller-controlled state payload is accepted. The Server derives it from current truth.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct SyncStateCommandInput {}

/// No secret material is accepted from the Panel. The Server resolves it from the vault later.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct SyncSecretCommandInput {}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum SyncStateCommandKind {
    SyncState,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum SyncSecretCommandKind {
    SyncSecret,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct SyncStateCommandRequest {
    kind: SyncStateCommandKind,
    device_id: String,
    reason_code: String,
    group_correlation_id: Option<String>,
    input_version: u32,
    input: SyncStateCommandInput,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct SyncSecretCommandRequest {
    kind: SyncSecretCommandKind,
    device_id: String,
    reason_code: String,
    group_correlation_id: Option<String>,
    input_version: u32,
    input: SyncSecretCommandInput,
}

/// Persisted request data supplied by the Control Panel.
///
/// Each `kind` has a separate closed object, so unknown or cross-kind input is rejected.
#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
#[serde(untagged)]
enum PutCommandRequest {
    SyncState(SyncStateCommandRequest),
    SyncSecret(SyncSecretCommandRequest),
}

/// The persistence state returned by the Command endpoint.
///
/// This is deliberately not a Device execution outcome.
#[allow(dead_code)]
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum CommandPersistenceState {
    Persisted,
}

/// A Command accepted into durable Server storage.
#[derive(Debug, Serialize, ToSchema)]
struct PersistedCommandResponse {
    #[schema(
        format = Uuid,
        pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$",
        example = "018f0e2e-8c1d-7c5e-8b12-3456789abcde"
    )]
    command_id: String,
    device_id: String,
    kind: CommandKind,
    state: CommandPersistenceState,
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Natsume V2",
        version = "2.5.0",
        description = "Rust-owned Phase 0 API contract. Version 2.5.0 is the retained API skeleton baseline and is independent of the 2.0.0 package version. The Phase 0 blueprint router exposes only GET /api/v2/health; imports, enrollment-request approval and declarative Command submission are frozen contracts, not currently mounted runtime routes. Catalogue routes in the architecture do not replace them implicitly."
    ),
    paths(
        get_health,
        create_csv_import,
        commit_csv_import,
        approve_enrollment,
        put_command
    ),
    components(schemas(
        HealthResponse,
        EnrollmentApprovalAccepted,
        OpenApiProblemDetails,
        CommandKind,
        SyncStateCommandInput,
        SyncSecretCommandInput,
        SyncStateCommandKind,
        SyncSecretCommandKind,
        SyncStateCommandRequest,
        SyncSecretCommandRequest,
        PutCommandRequest,
        CommandPersistenceState,
        PersistedCommandResponse
    ))
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
    path = "/api/v2/imports/{import_id}/actions/commit",
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
    path = "/api/v2/enrollment-requests/{request_id}/actions/approve",
    operation_id = "approveEnrollment",
    summary = "Approve Device Enrollment for Device Token-authenticated WSS control",
    params(("request_id" = Uuid, Path, description = "Enrollment request identifier")),
    responses((
        status = 202,
        description = "Enrollment approval accepted; the provisioning-window Enrollment transaction issues the Device Token and Gateway certificate (ADR-0033)",
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
            certificate_scope: "gateway".to_owned(),
        }),
    )
}

#[utoipa::path(
    put,
    path = "/api/v2/commands/{command_id}",
    operation_id = "putCommand",
    summary = "Persist a Panel-generated Command",
    description = "The Control Panel generates command_id before submission. It must be a canonical lowercase hyphenated UUIDv7. Retrying the same command_id with the same normalized request returns the persisted Command; a different request for that ID conflicts. A persisted response never represents Device execution.",
    params((
        "command_id" = String,
        Path,
        description = "Panel-generated canonical lowercase hyphenated UUIDv7 Command identifier",
        format = Uuid,
        pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$",
        example = "018f0e2e-8c1d-7c5e-8b12-3456789abcde"
    )),
    request_body = PutCommandRequest,
    responses(
        (
            status = 201,
            description = "The Command was persisted for the first time; Device execution is not implied.",
            body = PersistedCommandResponse
        ),
        (
            status = 200,
            description = "The same command_id and normalized request replayed an existing persisted Command; Device execution is not implied.",
            body = PersistedCommandResponse
        ),
        (
            status = 400,
            description = "The command_id is not a canonical lowercase hyphenated UUIDv7 (COMMAND_ID_INVALID). The response never echoes the invalid value.",
            body = OpenApiProblemDetails
        ),
        (
            status = 409,
            description = "The command_id is already bound to a different normalized request (COMMAND_REQUEST_CONFLICT).",
            body = OpenApiProblemDetails
        )
    )
)]
#[allow(dead_code)]
fn put_command(
    Path(_command_id): Path<String>,
    Json(_request): Json<PutCommandRequest>,
) -> StatusCode {
    StatusCode::CREATED
}

/// Returns the Axum router fragment for paths with an unambiguous matcher.
///
/// The current blueprint binary does not mount this router; the Server HTTP runtime owns
/// that later integration.
pub fn router() -> Router {
    Router::new().route("/api/v2/health", get(get_health))
}

/// Builds the server-owned `OpenAPI` document without reading a committed snapshot.
#[must_use]
pub fn openapi() -> OpenApiDocument {
    ApiDocument::openapi()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_request_rejects_unknown_top_level_and_input_fields() {
        let valid = r#"{"kind":"sync_secret","device_id":"device-1","reason_code":"operator_requested","input_version":1,"input":{}}"#;
        assert!(serde_json::from_str::<PutCommandRequest>(valid).is_ok());

        for invalid in [
            r#"{"kind":"sync_secret","device_id":"device-1","reason_code":"operator_requested","input_version":1,"input":{},"password_ciphertext":"canary"}"#,
            r#"{"kind":"sync_secret","device_id":"device-1","reason_code":"operator_requested","input_version":1,"input":{"token_value":"canary"}}"#,
        ] {
            assert!(serde_json::from_str::<PutCommandRequest>(invalid).is_err());
        }
    }

    #[test]
    fn command_request_openapi_branches_are_closed_objects() {
        let Ok(document) = serde_json::to_value(openapi()) else {
            panic!("OpenAPI document must serialize");
        };
        let Some(branches) = document
            .pointer("/components/schemas/PutCommandRequest/oneOf")
            .and_then(serde_json::Value::as_array)
        else {
            panic!("Command request oneOf branches must exist");
        };
        assert_eq!(branches.len(), 2);
        for branch in branches {
            let schema = branch
                .get("$ref")
                .and_then(serde_json::Value::as_str)
                .and_then(|reference| reference.strip_prefix('#'))
                .and_then(|pointer| document.pointer(pointer))
                .unwrap_or(branch);
            assert_eq!(
                schema.get("additionalProperties"),
                Some(&serde_json::Value::Bool(false))
            );
        }
    }
}
