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
        schema::{
            AdditionalProperties, ArrayItems, KnownFormat, ObjectBuilder, OneOfBuilder, Schema,
            Type,
        },
        security::{ApiKey, ApiKeyValue, SecurityRequirement, SecurityScheme},
    },
};

const INFO_DESCRIPTION: &str = "Mounted Stage 5B operation IDs: getHealth, createSession, getSession, deleteSession, listSeats, listAccounts, listDevices, listBindings, revokeDevice, disableDevice, getCsvImport, createCsvImport, commitCsvImport, discardCsvImport, getProvisioningWindow, openProvisioningWindow, closeProvisioningWindow, createEnrollmentRequest, listEnrollmentRequests, approveEnrollment, rejectEnrollment.\nDeclared but not mounted in Stage 5B operation IDs: putCommand.";
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
        crate::http::handler::import::discard_import,
        crate::http::handler::provisioning::get_provisioning_window,
        crate::http::handler::provisioning::open_provisioning_window,
        crate::http::handler::provisioning::close_provisioning_window,
        crate::http::handler::enrollment::create_enrollment_request,
        crate::http::handler::enrollment::list_enrollment_requests,
        crate::http::handler::enrollment::approve_enrollment_request,
        crate::http::handler::enrollment::reject_enrollment_request
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
        crate::http::handler::import::ImportCommitResponse,
        crate::http::handler::provisioning::ProvisioningWindowResponse,
        crate::http::handler::enrollment::EnrollmentRequest,
        crate::http::handler::enrollment::EnrollmentHardwareIdentityQuality,
        crate::http::handler::enrollment::EnrollmentIssuedResponse,
        crate::http::handler::enrollment::EnrollmentIssuedState,
        crate::http::handler::enrollment::EnrollmentPendingResponse,
        crate::http::handler::enrollment::EnrollmentPendingState,
        crate::http::handler::enrollment::EnrollmentRequestSummaryResponse,
        crate::http::handler::enrollment::EnrollmentActionResponse
    ))
)]
struct MountedDocument;

/// Builds the complete Stage 5B `OpenAPI` document.
#[must_use]
pub fn document() -> OpenApi {
    let mut mounted = MountedDocument::openapi();
    let mut components = mounted.components.take().unwrap_or_default();
    configure_components(&mut components);

    let mut paths = mounted.paths;
    paths.merge(declared_but_unmounted_paths());
    canonicalize_path_parameters(&mut paths);
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

fn configure_components(components: &mut utoipa::openapi::Components) {
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
    for schema_name in [
        "EnrollmentIssuedResponse",
        "EnrollmentPendingResponse",
        "EnrollmentActionResponse",
        "EnrollmentRequestSummary",
    ] {
        if let Some(RefOr::T(Schema::Object(schema))) = components.schemas.get_mut(schema_name) {
            schema.properties.insert(
                "enrollment_request_id".to_owned(),
                Ref::from_schema_name("CanonicalUuidV7").into(),
            );
        }
    }
    if let Some(RefOr::T(Schema::Object(schema))) =
        components.schemas.get_mut("EnrollmentIssuedResponse")
    {
        schema.properties.insert(
            "device_id".to_owned(),
            Ref::from_schema_name("CanonicalUuidV7").into(),
        );
        if let Some(RefOr::T(Schema::Array(chain))) = schema.properties.get_mut("gateway_chain_der")
            && let ArrayItems::RefOrSchema(items) = &mut chain.items
            && let RefOr::T(Schema::Object(item)) = items.as_mut()
        {
            item.format = Some(SchemaFormat::KnownFormat(KnownFormat::Byte));
        }
    }
    if let Some(RefOr::T(Schema::Object(schema))) =
        components.schemas.get_mut("EnrollmentRequestSummary")
    {
        schema.properties.insert(
            "resolved_device_id".to_owned(),
            OneOfBuilder::new()
                .item(ObjectBuilder::new().schema_type(Type::Null))
                .item(Ref::from_schema_name("CanonicalUuidV7"))
                .into(),
        );
    }
}

fn canonicalize_path_parameters(paths: &mut Paths) {
    for (path, parameter_name) in [
        ("/api/v2/devices/{device_id}/actions/revoke", "device_id"),
        ("/api/v2/devices/{device_id}/actions/disable", "device_id"),
        ("/api/v2/imports/{import_id}/actions/commit", "import_id"),
        ("/api/v2/imports/{import_id}/actions/discard", "import_id"),
        (
            "/api/v2/enrollment-requests/{request_id}/actions/approve",
            "request_id",
        ),
        (
            "/api/v2/enrollment-requests/{request_id}/actions/reject",
            "request_id",
        ),
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
            "/api/v2/commands/{command_id}",
            PathItem::new(HttpMethod::Put, put_command_operation()),
        )
        .build()
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
mod tests;
