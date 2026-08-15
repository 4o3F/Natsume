//! Rust-owned `OpenAPI` document for the Stage 5B Server surface.

use utoipa::{
    OpenApi as OpenApiTrait,
    openapi::{
        Content, OpenApi, OpenApiBuilder, Ref, RefOr, Required, SchemaFormat,
        header::HeaderBuilder,
        info::InfoBuilder,
        path::{HttpMethod, Operation, OperationBuilder, ParameterBuilder, ParameterIn, PathItem},
        path::{Paths, PathsBuilder},
        request_body::RequestBodyBuilder,
        response::Response,
        schema::{AdditionalProperties, KnownFormat, ObjectBuilder, Schema, Type},
        security::{ApiKey, ApiKeyValue, SecurityRequirement, SecurityScheme},
    },
};

const INFO_DESCRIPTION: &str = "Mounted Stage 5B operation IDs: getHealth, createSession, getSession, deleteSession, listSeats, listAccounts, listDevices, listBindings, revokeDevice, disableDevice, getCsvImport, createCsvImport, commitCsvImport, discardCsvImport.\nDeclared but not mounted in Stage 5B operation IDs: approveEnrollment, putCommand.";
const SESSION_COOKIE_SECURITY_SCHEME: &str = "sessionCookie";
const SESSION_COOKIE_NAME: &str = "__Secure-natsume_session";
const CANONICAL_UUID_V7_PATTERN: &str =
    "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";
const COMMAND_KIND_VALUES: [&str; 7] = [
    "sync_state",
    "sync_secret",
    "open_binding_prompt",
    "lock_session",
    "unlock_session",
    "terminate_session",
    "reset_home",
];
const COMMAND_DESCRIPTION: &str = "command_id must be a canonical lowercase hyphenated UUIDv7. The same canonical request, identified by its versioned domain-separated request fingerprint, replays the existing Command. A differing canonical request conflicts.";

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        crate::http::handler::health::health,
        crate::http::handler::session::create_session,
        crate::http::handler::session::read_session,
        crate::http::handler::session::delete_session,
        crate::http::handler::contest::list_seats,
        crate::http::handler::contest::list_accounts,
        crate::http::handler::contest::list_devices,
        crate::http::handler::contest::list_bindings,
        crate::http::handler::contest::revoke_device,
        crate::http::handler::contest::disable_device,
        crate::http::handler::import::get_import,
        crate::http::handler::import::create_import,
        crate::http::handler::import::commit_import,
        crate::http::handler::import::discard_import
    ),
    components(schemas(
        crate::http::handler::health::HealthResponse,
        crate::http::handler::session::SessionRequest,
        crate::http::handler::session::SessionResponse,
        crate::application::contest::SeatFacts,
        crate::application::contest::AccountFacts,
        crate::application::contest::DeviceFacts,
        crate::application::contest::BindingFacts,
        crate::http::handler::import::ImportMappingChangeResponse,
        crate::http::handler::import::ImportBindingImpactResponse,
        crate::http::handler::import::ImportRedactedDiff,
        crate::http::handler::import::ImportPreviewResponse,
        crate::http::handler::import::ImportPendingSummary,
        crate::http::handler::import::ImportPendingResponse,
        crate::http::handler::import::ImportCommitRequest,
        crate::http::handler::import::ImportCommitResponse
    ))
)]
struct MountedDocument;

/// Builds the complete Stage 5B `OpenAPI` document.
#[must_use]
pub fn document() -> OpenApi {
    let mut mounted = MountedDocument::openapi();
    let mut components = mounted.components.take().unwrap_or_default();
    components.schemas.insert(
        "PutCommandRequest".to_owned(),
        put_command_request_schema().into(),
    );
    components.schemas.insert(
        "CanonicalUuidV7".to_owned(),
        canonical_uuid_v7_schema().into(),
    );
    components
        .schemas
        .insert("ErrorResponse".to_owned(), error_response_schema().into());
    components.add_security_scheme(
        SESSION_COOKIE_SECURITY_SCHEME,
        SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(SESSION_COOKIE_NAME))),
    );
    if let Some(RefOr::T(Schema::Object(schema))) =
        components.schemas.get_mut("ImportPreviewResponse")
    {
        schema.properties.insert(
            "candidate_id".to_owned(),
            Ref::from_schema_name("CanonicalUuidV7").into(),
        );
    }
    if let Some(RefOr::T(Schema::Object(schema))) =
        components.schemas.get_mut("ImportPendingSummary")
    {
        schema.properties.insert(
            "candidate_id".to_owned(),
            Ref::from_schema_name("CanonicalUuidV7").into(),
        );
    }

    let mut paths = mounted.paths;
    paths.merge(declared_but_unmounted_paths());
    for (path, parameter_name) in [
        ("/api/v2/devices/{device_id}/actions/revoke", "device_id"),
        ("/api/v2/devices/{device_id}/actions/disable", "device_id"),
        ("/api/v2/imports/{import_id}/actions/commit", "import_id"),
        ("/api/v2/imports/{import_id}/actions/discard", "import_id"),
    ] {
        let Some(parameters) = paths
            .paths
            .get_mut(path)
            .and_then(|path_item| path_item.post.as_mut())
            .and_then(|operation| operation.parameters.as_mut())
        else {
            continue;
        };
        let Some(parameter) = parameters
            .iter_mut()
            .find(|parameter| parameter.name == parameter_name)
        else {
            continue;
        };
        parameter.schema = Some(Ref::from_schema_name("CanonicalUuidV7").into());
    }
    remove_operation_tags(&mut paths);
    enrich_responses(&mut paths);

    OpenApiBuilder::new()
        .info(
            InfoBuilder::new()
                .title("Natsume V2 Server API")
                .version("2.0.0")
                .description(Some(INFO_DESCRIPTION))
                .build(),
        )
        .paths(paths)
        .components(Some(components))
        .build()
}

fn remove_operation_tags(paths: &mut Paths) {
    for path in paths.paths.values_mut() {
        for operation in [
            path.get.as_mut(),
            path.put.as_mut(),
            path.post.as_mut(),
            path.delete.as_mut(),
            path.options.as_mut(),
            path.head.as_mut(),
            path.patch.as_mut(),
            path.trace.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            operation.tags = None;
        }
    }
}

fn declared_but_unmounted_paths() -> utoipa::openapi::path::Paths {
    PathsBuilder::new()
        .path(
            "/api/v2/enrollment-requests/{request_id}/actions/approve",
            PathItem::new(
                HttpMethod::Post,
                simple_operator_operation(
                    "approveEnrollment",
                    "Approve a Device enrollment request",
                    ("202", "Device enrollment approval accepted"),
                    Some("request_id"),
                ),
            ),
        )
        .path(
            "/api/v2/commands/{command_id}",
            PathItem::new(HttpMethod::Put, put_command_operation()),
        )
        .build()
}

fn simple_operator_operation(
    operation_id: &'static str,
    summary: &'static str,
    response: (&'static str, &'static str),
    path_parameter: Option<&'static str>,
) -> Operation {
    let operation = OperationBuilder::new()
        .operation_id(Some(operation_id))
        .summary(Some(summary))
        .security(session_cookie_requirement())
        .response(response.0, Response::new(response.1));
    match path_parameter {
        Some(name) => operation
            .parameter(
                ParameterBuilder::new()
                    .name(name)
                    .parameter_in(ParameterIn::Path)
                    .required(Required::True)
                    .schema(Some(ObjectBuilder::new().schema_type(Type::String)))
                    .build(),
            )
            .build(),
        None => operation.build(),
    }
}

fn put_command_operation() -> Operation {
    OperationBuilder::new()
        .operation_id(Some("putCommand"))
        .summary(Some("Create or replay a direct Device Command"))
        .description(Some(COMMAND_DESCRIPTION))
        .parameter(
            ParameterBuilder::new()
                .name("command_id")
                .parameter_in(ParameterIn::Path)
                .required(Required::True)
                .description(Some(
                    "Canonical lowercase hyphenated UUIDv7 supplied by the Panel",
                ))
                .schema(Some(Ref::from_schema_name("CanonicalUuidV7")))
                .build(),
        )
        .request_body(Some(
            RequestBodyBuilder::new()
                .required(Some(Required::True))
                .content(
                    "application/json",
                    Content::new(Some(Ref::from_schema_name("PutCommandRequest"))),
                )
                .build(),
        ))
        .security(session_cookie_requirement())
        .response("200", Response::new("Identical Command request replayed"))
        .response("201", Response::new("Command created"))
        .response("400", Response::new("Command ID is not canonical UUIDv7"))
        .response("401", Response::new("Session authentication failed"))
        .response("403", Response::new("Administrator role required"))
        .response("404", Response::new("Device does not exist"))
        .response("409", Response::new("Command request conflicts"))
        .response("500", Response::new("Internal failure"))
        .build()
}

fn session_cookie_requirement() -> SecurityRequirement {
    SecurityRequirement::new(SESSION_COOKIE_SECURITY_SCHEME, Vec::<String>::new())
}

fn canonical_uuid_v7_schema() -> ObjectBuilder {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid)))
        .pattern(Some(CANONICAL_UUID_V7_PATTERN))
}

fn put_command_request_schema() -> Schema {
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .property("device_id", Ref::from_schema_name("CanonicalUuidV7"))
        .property("group_correlation_id", uuid_schema())
        .property("payload", ObjectBuilder::new().schema_type(Type::Object))
        .property(
            "payload_version",
            ObjectBuilder::new()
                .schema_type(Type::Integer)
                .format(Some(SchemaFormat::KnownFormat(KnownFormat::Int32))),
        )
        .property(
            "kind",
            ObjectBuilder::new()
                .schema_type(Type::String)
                .enum_values(Some(COMMAND_KIND_VALUES)),
        )
        .property(
            "reason_code",
            ObjectBuilder::new().schema_type(Type::String),
        )
        .required("device_id")
        .required("payload")
        .required("payload_version")
        .required("kind")
        .additional_properties(Some(AdditionalProperties::FreeForm(false)))
        .build()
        .into()
}

fn uuid_schema() -> ObjectBuilder {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid)))
}

fn error_response_schema() -> Schema {
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .property("title", ObjectBuilder::new().schema_type(Type::String))
        .property("status", ObjectBuilder::new().schema_type(Type::Integer))
        .property("code", ObjectBuilder::new().schema_type(Type::String))
        .property("correlation_id", uuid_schema())
        .required("title")
        .required("status")
        .required("code")
        .required("correlation_id")
        .additional_properties(Some(AdditionalProperties::FreeForm(false)))
        .build()
        .into()
}

fn enrich_responses(paths: &mut utoipa::openapi::path::Paths) {
    for path_item in paths.paths.values_mut() {
        enrich_operation(path_item.get.as_mut());
        enrich_operation(path_item.post.as_mut());
        enrich_operation(path_item.put.as_mut());
        enrich_operation(path_item.delete.as_mut());
    }
}

fn enrich_operation(operation: Option<&mut Operation>) {
    let Some(operation) = operation else {
        return;
    };
    for (status, response) in &mut operation.responses.responses {
        let RefOr::T(response) = response else {
            continue;
        };
        response.headers.insert(
            "X-Correlation-Id".to_owned(),
            HeaderBuilder::new()
                .description(Some("Server-generated canonical UUIDv7 correlation ID"))
                .schema(uuid_schema())
                .build(),
        );
        if matches!(
            status.as_str(),
            "400" | "401" | "403" | "404" | "409" | "500"
        ) {
            response.content.insert(
                "application/json".to_owned(),
                Content::new(Some(Ref::from_schema_name("ErrorResponse"))),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        path::PathBuf,
    };

    use axum::{
        body::Body,
        http::{Method, Request, StatusCode, header},
    };
    use regex::{Regex, RegexBuilder};
    use serde_json::{Map, Value};
    use snafu::Snafu;
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::{
        db::{Database, DatabaseConfig},
        http,
    };

    use super::{COMMAND_KIND_VALUES, document};

    const UNMOUNTED_DESCRIPTION_PREFIX: &str =
        "Declared but not mounted in Stage 5B operation IDs: ";
    const FORBIDDEN_CREDENTIAL_KEY: &str = r"(?i)^(?:(?:\w*_)?private_key(?:_\w*)?|(?:\w*_)?pass(?:word|phrase)(?:_(?:value|plaintext|material|secret))?|(?:\w*_)?token(?:_(?:value|plaintext|material|secret))?|(?:\w*_)?secret(?:_(?:value|plaintext|material|key))?)$";
    const ALLOWED_CREDENTIAL_PATHS: [&str; 3] = [
        "/components/schemas/SessionRequest/properties/password",
        "/components/schemas/ImportPreviewResponse/properties/preview_token",
        "/components/schemas/ImportCommitRequest/properties/preview_token",
    ];
    type OperationTable = BTreeMap<(String, String), (String, BTreeSet<String>)>;

    #[test]
    fn operation_tables_and_response_sets_are_exact() -> Result<(), TestFailure> {
        let value = serialized_document()?;
        let records = operation_records(&value)?;
        let actual = records
            .iter()
            .map(|record| {
                (
                    (record.method.clone(), record.path.clone()),
                    (record.operation_id.clone(), record.responses.clone()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let expected = expected_operation_table();
        if actual != expected {
            return Err(TestFailure::OperationTableChanged);
        }
        Ok(())
    }

    fn expected_operation_table() -> OperationTable {
        let rows: &[(&str, &str, &str, &[&str])] = &[
            (
                "delete",
                "/api/v2/session",
                "deleteSession",
                &["204", "500"],
            ),
            ("get", "/api/v2/health", "getHealth", &["200", "500"]),
            (
                "get",
                "/api/v2/session",
                "getSession",
                &["200", "401", "500"],
            ),
            ("get", "/api/v2/seats", "listSeats", &["200", "401", "500"]),
            (
                "get",
                "/api/v2/accounts",
                "listAccounts",
                &["200", "401", "500"],
            ),
            (
                "get",
                "/api/v2/devices",
                "listDevices",
                &["200", "401", "500"],
            ),
            (
                "get",
                "/api/v2/bindings",
                "listBindings",
                &["200", "401", "500"],
            ),
            (
                "get",
                "/api/v2/imports",
                "getCsvImport",
                &["200", "401", "403", "500"],
            ),
            (
                "post",
                "/api/v2/devices/{device_id}/actions/revoke",
                "revokeDevice",
                &["200", "400", "401", "403", "404", "500"],
            ),
            (
                "post",
                "/api/v2/devices/{device_id}/actions/disable",
                "disableDevice",
                &["200", "400", "401", "403", "404", "500"],
            ),
            (
                "post",
                "/api/v2/session",
                "createSession",
                &["200", "400", "401", "413", "500"],
            ),
            (
                "post",
                "/api/v2/imports",
                "createCsvImport",
                &["201", "400", "401", "403", "409", "413", "500"],
            ),
            (
                "post",
                "/api/v2/imports/{import_id}/actions/commit",
                "commitCsvImport",
                &["200", "400", "401", "403", "404", "409", "413", "500"],
            ),
            (
                "post",
                "/api/v2/imports/{import_id}/actions/discard",
                "discardCsvImport",
                &["204", "400", "401", "403", "404", "500"],
            ),
            (
                "post",
                "/api/v2/enrollment-requests/{request_id}/actions/approve",
                "approveEnrollment",
                &["202"],
            ),
            (
                "put",
                "/api/v2/commands/{command_id}",
                "putCommand",
                &["200", "201", "400", "401", "403", "404", "409", "500"],
            ),
        ];
        rows.iter()
            .map(|(method, path, operation_id, statuses)| {
                (
                    ((*method).to_owned(), (*path).to_owned()),
                    ((*operation_id).to_owned(), string_set(statuses)),
                )
            })
            .collect()
    }

    #[test]
    fn operation_ids_are_unique() -> Result<(), TestFailure> {
        let value = serialized_document()?;
        let records = operation_records(&value)?;
        let unique = records
            .iter()
            .map(|record| record.operation_id.as_str())
            .collect::<BTreeSet<_>>();
        if unique.len() != records.len() {
            return Err(TestFailure::OperationIdWasDuplicated);
        }
        Ok(())
    }

    #[test]
    fn internal_module_paths_and_operation_tags_are_absent() -> Result<(), TestFailure> {
        let value = serialized_document()?;
        if value_contains_string_prefix(&value, "crate::") {
            return Err(TestFailure::InternalModulePathEscaped);
        }
        for record in operation_records(&value)? {
            if operation_at(&value, &record.path, &record.method)?.contains_key("tags") {
                return Err(TestFailure::InternalModulePathEscaped);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn mounted_document_set_equals_the_live_router() -> Result<(), TestFailure> {
        let value = serialized_document()?;
        let records = operation_records(&value)?;
        let described_unmounted = described_unmounted_ids(&value)?;
        let documented_mounted = records
            .iter()
            .filter(|record| !described_unmounted.contains(&record.operation_id))
            .map(OperationRecord::method_path)
            .collect::<BTreeSet<_>>();
        let router_mounted = probe_live_router(&records).await?;
        if documented_mounted != router_mounted {
            return Err(TestFailure::RouterAndDocumentDiverged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn description_names_every_and_only_unmounted_operation() -> Result<(), TestFailure> {
        let value = serialized_document()?;
        let records = operation_records(&value)?;
        let router_mounted = probe_live_router(&records).await?;
        let computed_unmounted = records
            .iter()
            .filter(|record| !router_mounted.contains(&record.method_path()))
            .map(|record| record.operation_id.clone())
            .collect::<BTreeSet<_>>();
        if described_unmounted_ids(&value)? != computed_unmounted {
            return Err(TestFailure::UnmountedDescriptionDiverged);
        }
        Ok(())
    }

    #[test]
    fn info_description_is_exact() -> Result<(), TestFailure> {
        let value = serialized_document()?;
        let description = value
            .pointer("/info/description")
            .and_then(Value::as_str)
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        if description
            != "Mounted Stage 5B operation IDs: getHealth, createSession, getSession, deleteSession, listSeats, listAccounts, listDevices, listBindings, revokeDevice, disableDevice, getCsvImport, createCsvImport, commitCsvImport, discardCsvImport.\nDeclared but not mounted in Stage 5B operation IDs: approveEnrollment, putCommand."
        {
            return Err(TestFailure::InfoDescriptionChanged);
        }
        Ok(())
    }

    #[test]
    fn device_lifecycle_path_parameters_are_exact() -> Result<(), TestFailure> {
        let value = serialized_document()?;
        let canonical_uuid_v7 = value
            .pointer("/components/schemas/CanonicalUuidV7")
            .and_then(Value::as_object)
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        if canonical_uuid_v7.get("type").and_then(Value::as_str) != Some("string")
            || canonical_uuid_v7.get("format").and_then(Value::as_str) != Some("uuid")
            || canonical_uuid_v7.get("pattern").and_then(Value::as_str)
                != Some("^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
        {
            return Err(TestFailure::LifecyclePathContractChanged);
        }
        for path in [
            "/api/v2/devices/{device_id}/actions/revoke",
            "/api/v2/devices/{device_id}/actions/disable",
        ] {
            let parameters = operation_at(&value, path, "post")?
                .get("parameters")
                .and_then(Value::as_array)
                .ok_or(TestFailure::DocumentShapeInvalid)?;
            let parameter = parameters
                .iter()
                .find(|parameter| {
                    parameter.get("name").and_then(Value::as_str) == Some("device_id")
                })
                .ok_or(TestFailure::LifecyclePathContractChanged)?;
            if parameter.get("in").and_then(Value::as_str) != Some("path")
                || parameter.get("required").and_then(Value::as_bool) != Some(true)
                || parameter.pointer("/schema/$ref").and_then(Value::as_str)
                    != Some("#/components/schemas/CanonicalUuidV7")
            {
                return Err(TestFailure::LifecyclePathContractChanged);
            }
        }
        Ok(())
    }

    #[test]
    fn import_paths_and_schemas_are_closed_and_exact() -> Result<(), TestFailure> {
        let value = serialized_document()?;
        assert_import_operations(&value)?;
        assert_import_schemas(&value)
    }

    fn assert_import_operations(value: &Value) -> Result<(), TestFailure> {
        for path in [
            "/api/v2/imports/{import_id}/actions/commit",
            "/api/v2/imports/{import_id}/actions/discard",
        ] {
            let parameter = operation_at(value, path, "post")?
                .get("parameters")
                .and_then(Value::as_array)
                .and_then(|parameters| {
                    parameters.iter().find(|parameter| {
                        parameter.get("name").and_then(Value::as_str) == Some("import_id")
                    })
                })
                .ok_or(TestFailure::ImportContractChanged)?;
            if parameter.get("in").and_then(Value::as_str) != Some("path")
                || parameter.get("required").and_then(Value::as_bool) != Some(true)
                || parameter.pointer("/schema/$ref").and_then(Value::as_str)
                    != Some("#/components/schemas/CanonicalUuidV7")
            {
                return Err(TestFailure::ImportContractChanged);
            }
        }

        let upload = operation_at(value, "/api/v2/imports", "post")?;
        let read = operation_at(value, "/api/v2/imports", "get")?;
        let commit = operation_at(value, "/api/v2/imports/{import_id}/actions/commit", "post")?;
        let discard = operation_at(value, "/api/v2/imports/{import_id}/actions/discard", "post")?;
        if nested_value(
            upload,
            &["requestBody", "content", "text/csv", "schema", "type"],
        )
        .and_then(Value::as_str)
            != Some("string")
            || read.get("requestBody").is_some()
            || nested_value(
                read,
                &[
                    "responses",
                    "200",
                    "content",
                    "application/json",
                    "schema",
                    "$ref",
                ],
            )
            .and_then(Value::as_str)
                != Some("#/components/schemas/ImportPendingResponse")
            || nested_value(
                commit,
                &[
                    "requestBody",
                    "content",
                    "application/json",
                    "schema",
                    "$ref",
                ],
            )
            .and_then(Value::as_str)
                != Some("#/components/schemas/ImportCommitRequest")
            || discard.get("requestBody").is_some()
            || nested_value(
                upload,
                &[
                    "responses",
                    "201",
                    "content",
                    "application/json",
                    "schema",
                    "$ref",
                ],
            )
            .and_then(Value::as_str)
                != Some("#/components/schemas/ImportPreviewResponse")
            || nested_value(
                commit,
                &[
                    "responses",
                    "200",
                    "content",
                    "application/json",
                    "schema",
                    "$ref",
                ],
            )
            .and_then(Value::as_str)
                != Some("#/components/schemas/ImportCommitResponse")
        {
            return Err(TestFailure::ImportContractChanged);
        }
        Ok(())
    }

    fn assert_import_schemas(value: &Value) -> Result<(), TestFailure> {
        assert_import_preview_schema(value)?;
        assert_import_pending_schemas(value)?;
        assert_import_diff_schema(value)?;
        assert_import_mapping_schemas(value)?;
        assert_import_commit_schemas(value)
    }

    fn assert_import_pending_schemas(value: &Value) -> Result<(), TestFailure> {
        let response = schema_object(value, "ImportPendingResponse")?;
        let response_properties = schema_properties(response)?;
        if property_names(response_properties) != BTreeSet::from(["pending"])
            || required_property_names(response)? != BTreeSet::from(["pending"])
            || response
                .get("additionalProperties")
                .and_then(Value::as_bool)
                != Some(false)
            || !value_contains_string(
                response_properties
                    .get("pending")
                    .ok_or(TestFailure::ImportContractChanged)?,
                "null",
            )
        {
            return Err(TestFailure::ImportContractChanged);
        }

        let summary = schema_object(value, "ImportPendingSummary")?;
        let properties = schema_properties(summary)?;
        if property_names(properties)
            != BTreeSet::from([
                "baseline_binding_revision",
                "baseline_configuration_revision",
                "candidate_id",
                "diff",
                "expires_at",
            ])
            || required_property_names(summary)? != property_names(properties)
            || summary.get("additionalProperties").and_then(Value::as_bool) != Some(false)
            || properties
                .get("candidate_id")
                .and_then(|property| property.get("$ref"))
                .and_then(Value::as_str)
                != Some("#/components/schemas/CanonicalUuidV7")
            || properties
                .get("baseline_configuration_revision")
                .and_then(|property| property.get("format"))
                .and_then(Value::as_str)
                != Some("int64")
            || properties
                .get("baseline_binding_revision")
                .and_then(|property| property.get("format"))
                .and_then(Value::as_str)
                != Some("int64")
            || properties
                .get("diff")
                .and_then(|property| property.get("$ref"))
                .and_then(Value::as_str)
                != Some("#/components/schemas/ImportRedactedDiff")
        {
            return Err(TestFailure::ImportContractChanged);
        }
        Ok(())
    }

    fn assert_import_preview_schema(value: &Value) -> Result<(), TestFailure> {
        let preview = schema_object(value, "ImportPreviewResponse")?;
        let preview_properties = schema_properties(preview)?;
        if property_names(preview_properties)
            != BTreeSet::from([
                "baseline_binding_revision",
                "baseline_configuration_revision",
                "candidate_id",
                "diff",
                "expires_at",
                "preview_token",
            ])
            || preview.get("additionalProperties").and_then(Value::as_bool) != Some(false)
            || preview_properties
                .get("candidate_id")
                .and_then(|property| property.get("$ref"))
                .and_then(Value::as_str)
                != Some("#/components/schemas/CanonicalUuidV7")
            || preview_properties
                .get("baseline_configuration_revision")
                .and_then(|property| property.get("format"))
                .and_then(Value::as_str)
                != Some("int64")
            || preview_properties
                .get("baseline_binding_revision")
                .and_then(|property| property.get("format"))
                .and_then(Value::as_str)
                != Some("int64")
            || preview_properties
                .get("preview_token")
                .and_then(|property| property.get("pattern"))
                .and_then(Value::as_str)
                != Some("^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$")
            || preview_properties
                .get("expires_at")
                .and_then(|property| property.get("description"))
                .and_then(Value::as_str)
                != Some("RFC 3339 UTC timestamp with a trailing Z.")
        {
            return Err(TestFailure::ImportContractChanged);
        }
        Ok(())
    }

    fn assert_import_diff_schema(value: &Value) -> Result<(), TestFailure> {
        let diff = schema_object(value, "ImportRedactedDiff")?;
        let properties = schema_properties(diff)?;
        if property_names(properties)
            != BTreeSet::from([
                "affected_account_count",
                "binding_impacts",
                "mappings_changed",
                "seats_added",
                "seats_removed",
                "unchanged_count",
            ])
            || diff.get("additionalProperties").and_then(Value::as_bool) != Some(false)
        {
            return Err(TestFailure::ImportContractChanged);
        }
        for name in ["unchanged_count", "affected_account_count"] {
            let property = properties
                .get(name)
                .ok_or(TestFailure::ImportContractChanged)?;
            if property.get("type").and_then(Value::as_str) != Some("integer")
                || property.get("minimum").and_then(Value::as_u64) != Some(0)
            {
                return Err(TestFailure::ImportContractChanged);
            }
        }
        Ok(())
    }

    fn assert_import_mapping_schemas(value: &Value) -> Result<(), TestFailure> {
        let mapping = schema_object(value, "ImportMappingChangeResponse")?;
        let mapping_properties = schema_properties(mapping)?;
        let binding = schema_object(value, "ImportBindingImpactResponse")?;
        if property_names(mapping_properties)
            != BTreeSet::from([
                "candidate_domjudge_username",
                "current_domjudge_username",
                "seat_code",
            ])
            || required_property_names(mapping)?
                != BTreeSet::from([
                    "candidate_domjudge_username",
                    "current_domjudge_username",
                    "seat_code",
                ])
            || property_names(schema_properties(binding)?)
                != BTreeSet::from(["device_id", "seat_code"])
            || !value_contains_string(
                mapping_properties
                    .get("current_domjudge_username")
                    .ok_or(TestFailure::ImportContractChanged)?,
                "null",
            )
        {
            return Err(TestFailure::ImportContractChanged);
        }
        Ok(())
    }

    fn assert_import_commit_schemas(value: &Value) -> Result<(), TestFailure> {
        for (schema_name, expected_properties) in [
            ("ImportCommitRequest", BTreeSet::from(["preview_token"])),
            (
                "ImportCommitResponse",
                BTreeSet::from(["binding_revision", "configuration_revision"]),
            ),
        ] {
            let schema = schema_object(value, schema_name)?;
            if property_names(schema_properties(schema)?) != expected_properties
                || schema.get("additionalProperties").and_then(Value::as_bool) != Some(false)
            {
                return Err(TestFailure::ImportContractChanged);
            }
        }
        let commit_request = schema_properties(schema_object(value, "ImportCommitRequest")?)?;
        if commit_request
            .get("preview_token")
            .and_then(|property| property.get("pattern"))
            .and_then(Value::as_str)
            != Some("^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$")
        {
            return Err(TestFailure::ImportContractChanged);
        }
        Ok(())
    }

    #[test]
    fn put_command_contract_is_closed_and_exact() -> Result<(), TestFailure> {
        let value = serialized_document()?;
        let operation = operation_at(&value, "/api/v2/commands/{command_id}", "put")?;
        if operation.get("description").and_then(Value::as_str)
            != Some(
                "command_id must be a canonical lowercase hyphenated UUIDv7. The same canonical request, identified by its versioned domain-separated request fingerprint, replays the existing Command. A differing canonical request conflicts.",
            )
        {
            return Err(TestFailure::CommandDescriptionChanged);
        }
        let parameter = operation
            .get("parameters")
            .and_then(Value::as_array)
            .and_then(|parameters| {
                parameters.iter().find(|parameter| {
                    parameter.get("name").and_then(Value::as_str) == Some("command_id")
                })
            })
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        let parameter_schema = parameter
            .get("schema")
            .and_then(Value::as_object)
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        if parameter.get("in").and_then(Value::as_str) != Some("path")
            || parameter.get("required").and_then(Value::as_bool) != Some(true)
            || parameter_schema.get("$ref").and_then(Value::as_str)
                != Some("#/components/schemas/CanonicalUuidV7")
        {
            return Err(TestFailure::CommandIdContractChanged);
        }

        let schema = value
            .pointer("/components/schemas/PutCommandRequest")
            .and_then(Value::as_object)
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        let property_set = properties
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let expected_properties = BTreeSet::from([
            "device_id",
            "group_correlation_id",
            "kind",
            "payload",
            "payload_version",
            "reason_code",
        ]);
        let kind = properties
            .get("kind")
            .and_then(Value::as_object)
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        let actual_kind_values = kind
            .get("enum")
            .and_then(Value::as_array)
            .and_then(|values| values.iter().map(Value::as_str).collect::<Option<Vec<_>>>());
        if property_set != expected_properties
            || schema.get("additionalProperties").and_then(Value::as_bool) != Some(false)
            || kind.get("type").and_then(Value::as_str) != Some("string")
            || actual_kind_values != Some(COMMAND_KIND_VALUES.to_vec())
            || properties
                .get("device_id")
                .and_then(Value::as_object)
                .and_then(|device_id| device_id.get("$ref"))
                .and_then(Value::as_str)
                != Some("#/components/schemas/CanonicalUuidV7")
        {
            return Err(TestFailure::CommandRequestContractChanged);
        }
        Ok(())
    }

    #[test]
    fn password_and_cookie_security_metadata_are_secret_safe() -> Result<(), TestFailure> {
        let value = serialized_document()?;
        let password = value
            .pointer("/components/schemas/SessionRequest/properties/password")
            .and_then(Value::as_object)
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        let request_ref = operation_at(&value, "/api/v2/session", "post")?
            .get("requestBody")
            .and_then(|request_body| request_body.get("content"))
            .and_then(|content| content.get("application/json"))
            .and_then(|media_type| media_type.get("schema"))
            .and_then(|schema| schema.get("$ref"))
            .and_then(Value::as_str);
        if password.get("writeOnly").and_then(Value::as_bool) != Some(true)
            || password.contains_key("example")
            || password.contains_key("examples")
            || password.contains_key("default")
            || request_ref != Some("#/components/schemas/SessionRequest")
        {
            return Err(TestFailure::PasswordSchemaWasNotWriteOnly);
        }
        for record in operation_records(&value)? {
            let operation = operation_at(&value, &record.path, &record.method)?;
            let responses = operation
                .get("responses")
                .ok_or(TestFailure::DocumentShapeInvalid)?;
            if value_contains_string(responses, "#/components/schemas/SessionRequest") {
                return Err(TestFailure::PasswordSchemaEscapedIntoResponse);
            }
            if record.operation_id != "createSession"
                && operation.get("requestBody").is_some_and(|request_body| {
                    value_contains_string(request_body, "#/components/schemas/SessionRequest")
                })
            {
                return Err(TestFailure::PasswordSchemaEscapedIntoRequest);
            }
        }
        let security = value
            .pointer("/components/securitySchemes/sessionCookie")
            .and_then(Value::as_object)
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        if security.get("type").and_then(Value::as_str) != Some("apiKey")
            || security.get("in").and_then(Value::as_str) != Some("cookie")
            || security.get("name").and_then(Value::as_str) != Some("__Secure-natsume_session")
            || security.contains_key("value")
            || security.contains_key("example")
            || security.contains_key("default")
        {
            return Err(TestFailure::CookieSecuritySchemeChanged);
        }
        Ok(())
    }

    #[test]
    fn error_response_shape_and_correlation_headers_match_stage_four() -> Result<(), TestFailure> {
        let value = serialized_document()?;
        let error_response = value
            .pointer("/components/schemas/ErrorResponse")
            .and_then(Value::as_object)
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        let properties = error_response
            .get("properties")
            .and_then(Value::as_object)
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        let required = error_response
            .get("required")
            .and_then(Value::as_array)
            .ok_or(TestFailure::DocumentShapeInvalid)?
            .iter()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        let frozen_fields = BTreeSet::from(["title", "status", "code", "correlation_id"]);
        if properties
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != frozen_fields
            || required != frozen_fields
            || error_response
                .get("additionalProperties")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err(TestFailure::ErrorResponseSchemaChanged);
        }
        for record in operation_records(&value)? {
            let responses = operation_at(&value, &record.path, &record.method)?
                .get("responses")
                .and_then(Value::as_object)
                .ok_or(TestFailure::DocumentShapeInvalid)?;
            for (status, response) in responses {
                if response.pointer("/headers/X-Correlation-Id").is_none() {
                    return Err(TestFailure::CorrelationHeaderWasOmitted);
                }
                let has_error_response = response
                    .pointer("/content/application~1json/schema/$ref")
                    .and_then(Value::as_str)
                    == Some("#/components/schemas/ErrorResponse");
                if matches!(
                    status.as_str(),
                    "400" | "401" | "403" | "404" | "409" | "500"
                ) != has_error_response
                {
                    return Err(TestFailure::ErrorResponseMappingChanged);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn recursive_secret_key_scan_matches_the_web_rule() -> Result<(), TestFailure> {
        let value = serialized_document()?;
        let pattern = RegexBuilder::new(FORBIDDEN_CREDENTIAL_KEY)
            .unicode(false)
            .build()
            .map_err(|_| TestFailure::SecretPatternInvalid)?;
        scan_credential_keys(&value, "", &pattern)?;

        for key in [
            "private_key",
            "gateway_private_key_der",
            "signing_private_key",
            "private_key_pem",
            "password",
            "domjudge_password",
            "password_value",
            "passphrase",
            "token",
            "device_token",
            "raw_token",
            "access_token",
            "enrollment_token",
            "api_token",
            "session_token",
            "auth_token",
            "token_value",
            "secret",
            "client_secret",
            "shared_secret",
            "secret_key",
        ] {
            if !pattern.is_match(key) {
                return Err(TestFailure::SecretPatternDrifted);
            }
        }
        for key in [
            "gateway_csr_der",
            "gateway_leaf_der",
            "gateway_chain_der",
            "gateway_spki_sha256",
            "token_hash",
            "password_hash",
            "device_token_id",
            "public_key",
            "serial",
            "not_after",
        ] {
            if pattern.is_match(key) {
                return Err(TestFailure::SecretPatternDrifted);
            }
        }
        Ok(())
    }

    fn serialized_document() -> Result<Value, TestFailure> {
        serde_json::to_value(document()).map_err(|_| TestFailure::SerializationFailed)
    }

    fn operation_records(value: &Value) -> Result<Vec<OperationRecord>, TestFailure> {
        let paths = value
            .get("paths")
            .and_then(Value::as_object)
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        let mut records = Vec::new();
        for (path, item) in paths {
            let item = item.as_object().ok_or(TestFailure::DocumentShapeInvalid)?;
            for method in ["get", "head", "post", "put", "delete"] {
                let Some(operation) = item.get(method) else {
                    continue;
                };
                let operation = operation
                    .as_object()
                    .ok_or(TestFailure::DocumentShapeInvalid)?;
                let operation_id = operation
                    .get("operationId")
                    .and_then(Value::as_str)
                    .ok_or(TestFailure::DocumentShapeInvalid)?;
                let responses = operation
                    .get("responses")
                    .and_then(Value::as_object)
                    .ok_or(TestFailure::DocumentShapeInvalid)?
                    .keys()
                    .cloned()
                    .collect();
                records.push(OperationRecord {
                    method: method.to_owned(),
                    path: path.clone(),
                    operation_id: operation_id.to_owned(),
                    responses,
                });
            }
        }
        Ok(records)
    }

    fn operation_at<'a>(
        value: &'a Value,
        path: &str,
        method: &str,
    ) -> Result<&'a Map<String, Value>, TestFailure> {
        value
            .get("paths")
            .and_then(|paths| paths.get(path))
            .and_then(|item| item.get(method))
            .and_then(Value::as_object)
            .ok_or(TestFailure::DocumentShapeInvalid)
    }

    fn schema_object<'a>(
        value: &'a Value,
        name: &str,
    ) -> Result<&'a Map<String, Value>, TestFailure> {
        value
            .get("components")
            .and_then(|components| components.get("schemas"))
            .and_then(|schemas| schemas.get(name))
            .and_then(Value::as_object)
            .ok_or(TestFailure::ImportContractChanged)
    }

    fn schema_properties(schema: &Map<String, Value>) -> Result<&Map<String, Value>, TestFailure> {
        schema
            .get("properties")
            .and_then(Value::as_object)
            .ok_or(TestFailure::ImportContractChanged)
    }

    fn property_names(properties: &Map<String, Value>) -> BTreeSet<&str> {
        properties.keys().map(String::as_str).collect()
    }

    fn required_property_names(schema: &Map<String, Value>) -> Result<BTreeSet<&str>, TestFailure> {
        schema
            .get("required")
            .and_then(Value::as_array)
            .and_then(|required| required.iter().map(Value::as_str).collect())
            .ok_or(TestFailure::ImportContractChanged)
    }

    fn nested_value<'a>(object: &'a Map<String, Value>, path: &[&str]) -> Option<&'a Value> {
        let (first, rest) = path.split_first()?;
        let mut value = object.get(*first)?;
        for key in rest {
            value = value.as_object()?.get(*key)?;
        }
        Some(value)
    }

    fn described_unmounted_ids(value: &Value) -> Result<BTreeSet<String>, TestFailure> {
        let description = value
            .pointer("/info/description")
            .and_then(Value::as_str)
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        let line = description
            .lines()
            .find_map(|line| line.strip_prefix(UNMOUNTED_DESCRIPTION_PREFIX))
            .and_then(|line| line.strip_suffix('.'))
            .ok_or(TestFailure::DocumentShapeInvalid)?;
        Ok(line.split(", ").map(str::to_owned).collect())
    }

    async fn probe_live_router(
        records: &[OperationRecord],
    ) -> Result<BTreeSet<(String, String)>, TestFailure> {
        let fixture = TestDatabase::new().await?;
        let application = http::router(
            fixture.database.clone(),
            http::tests::unused_vault_master_key(),
            http::tests::unused_web_root(),
        );
        let mut paths = records
            .iter()
            .map(|record| record.path.clone())
            .collect::<BTreeSet<_>>();
        paths.extend([
            "/api/v2/seats".to_owned(),
            "/api/v2/accounts".to_owned(),
            "/api/v2/devices".to_owned(),
            "/api/v2/bindings".to_owned(),
            "/api/v2/devices/{device_id}/actions/revoke".to_owned(),
            "/api/v2/devices/{device_id}/actions/disable".to_owned(),
        ]);

        let mut mounted = BTreeSet::new();
        for path in paths {
            let concrete_path = concrete_path(&path);
            for method in ["get", "head", "post", "put", "delete"] {
                let request = probe_request(method, &concrete_path)?;
                let response = application
                    .clone()
                    .oneshot(request)
                    .await
                    .map_err(|_| TestFailure::RouterProbeFailed)?;
                if response.status() != StatusCode::NOT_FOUND
                    && response.status() != StatusCode::METHOD_NOT_ALLOWED
                {
                    mounted.insert((method.to_owned(), path.clone()));
                }
            }
        }
        Ok(mounted)
    }

    fn probe_request(method: &str, path: &str) -> Result<Request<Body>, TestFailure> {
        let method = match method {
            "get" => Method::GET,
            "head" => Method::HEAD,
            "post" => Method::POST,
            "put" => Method::PUT,
            "delete" => Method::DELETE,
            _ => return Err(TestFailure::DocumentShapeInvalid),
        };
        let is_session_post = path == "/api/v2/session" && method == Method::POST;
        let mut request = Request::builder().method(method).uri(path);
        let body = if is_session_post {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from("{}")
        } else {
            Body::empty()
        };
        request
            .body(body)
            .map_err(|_| TestFailure::RouterProbeFailed)
    }

    fn concrete_path(path: &str) -> String {
        path.replace("{command_id}", "01900000-0000-7000-8000-000000000000")
            .replace("{device_id}", "01900000-0000-7000-8000-000000000000")
            .replace("{import_id}", "01900000-0000-7000-8000-000000000000")
            .replace("{request_id}", "01900000-0000-7000-8000-000000000000")
    }

    fn scan_credential_keys(
        value: &Value,
        parent_path: &str,
        pattern: &Regex,
    ) -> Result<(), TestFailure> {
        match value {
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    scan_credential_keys(value, &format!("{parent_path}/{index}"), pattern)?;
                }
            }
            Value::Object(object) => {
                for (key, value) in object {
                    let path = format!("{parent_path}/{key}");
                    let normalized = key.replace('-', "_");
                    if pattern.is_match(&normalized)
                        && !ALLOWED_CREDENTIAL_PATHS.contains(&path.as_str())
                    {
                        return Err(TestFailure::SecretKeyEscaped);
                    }
                    scan_credential_keys(value, &path, pattern)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn value_contains_string(value: &Value, needle: &str) -> bool {
        match value {
            Value::Array(values) => values
                .iter()
                .any(|value| value_contains_string(value, needle)),
            Value::Object(object) => object
                .values()
                .any(|value| value_contains_string(value, needle)),
            Value::String(value) => value == needle,
            _ => false,
        }
    }

    fn value_contains_string_prefix(value: &Value, prefix: &str) -> bool {
        match value {
            Value::Array(values) => values
                .iter()
                .any(|value| value_contains_string_prefix(value, prefix)),
            Value::Object(object) => object
                .values()
                .any(|value| value_contains_string_prefix(value, prefix)),
            Value::String(value) => value.starts_with(prefix),
            _ => false,
        }
    }

    fn string_set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    struct OperationRecord {
        method: String,
        path: String,
        operation_id: String,
        responses: BTreeSet<String>,
    }

    impl OperationRecord {
        fn method_path(&self) -> (String, String) {
            (self.method.clone(), self.path.clone())
        }
    }

    struct TestDatabase {
        database: Database,
        path: PathBuf,
    }

    impl TestDatabase {
        async fn new() -> Result<Self, TestFailure> {
            let path = std::env::temp_dir().join(format!(
                "natsume-openapi-router-test-{}.sqlite3",
                Uuid::now_v7()
            ));
            let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
                .await
                .map_err(|_| TestFailure::FixtureFailed)?;
            Ok(Self { database, path })
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            let _database_result = fs::remove_file(&self.path);
            let _wal_result = fs::remove_file(format!("{}-wal", self.path.display()));
            let _shm_result = fs::remove_file(format!("{}-shm", self.path.display()));
        }
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the OpenAPI document could not be serialized"))]
        SerializationFailed,
        #[snafu(display("the OpenAPI document shape was invalid"))]
        DocumentShapeInvalid,
        #[snafu(display("the OpenAPI operation table changed"))]
        OperationTableChanged,
        #[snafu(display("an OpenAPI operation ID was duplicated"))]
        OperationIdWasDuplicated,
        #[snafu(display("an internal module path or operation tag escaped into OpenAPI"))]
        InternalModulePathEscaped,
        #[snafu(display("the live router and mounted OpenAPI document diverged"))]
        RouterAndDocumentDiverged,
        #[snafu(display("the declared-but-unmounted description diverged"))]
        UnmountedDescriptionDiverged,
        #[snafu(display("the OpenAPI info description changed"))]
        InfoDescriptionChanged,
        #[snafu(display("the Device lifecycle path parameter contract changed"))]
        LifecyclePathContractChanged,
        #[snafu(display("the import OpenAPI contract changed"))]
        ImportContractChanged,
        #[snafu(display("the Command description changed"))]
        CommandDescriptionChanged,
        #[snafu(display("the Command ID contract changed"))]
        CommandIdContractChanged,
        #[snafu(display("the Command request contract changed"))]
        CommandRequestContractChanged,
        #[snafu(display("the session password schema was not write-only"))]
        PasswordSchemaWasNotWriteOnly,
        #[snafu(display("the session password schema escaped into a response"))]
        PasswordSchemaEscapedIntoResponse,
        #[snafu(display("the session password schema escaped into another request"))]
        PasswordSchemaEscapedIntoRequest,
        #[snafu(display("the session cookie security scheme changed"))]
        CookieSecuritySchemeChanged,
        #[snafu(display("the error response schema changed"))]
        ErrorResponseSchemaChanged,
        #[snafu(display("an OpenAPI response omitted the correlation header"))]
        CorrelationHeaderWasOmitted,
        #[snafu(display("the OpenAPI error response mapping changed"))]
        ErrorResponseMappingChanged,
        #[snafu(display("the secret-key pattern was invalid"))]
        SecretPatternInvalid,
        #[snafu(display("the secret-key pattern drifted from the Web rule"))]
        SecretPatternDrifted,
        #[snafu(display("a forbidden secret-shaped key escaped into OpenAPI"))]
        SecretKeyEscaped,
        #[snafu(display("the OpenAPI router probe failed"))]
        RouterProbeFailed,
        #[snafu(display("the OpenAPI test fixture failed"))]
        FixtureFailed,
    }
}
