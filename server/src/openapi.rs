//! Rust-owned `OpenAPI` document for the Stage 5B Server surface.

use utoipa::{
    OpenApi as OpenApiTrait,
    openapi::{
        Content, OpenApi, OpenApiBuilder, Ref, RefOr, SchemaFormat,
        info::InfoBuilder,
        path::{Operation, Paths},
        schema::{AdditionalProperties, KnownFormat, ObjectBuilder, Schema, Type},
        security::{ApiKey, ApiKeyValue, SecurityScheme},
    },
};

const INFO_DESCRIPTION: &str = "Mounted WP8 operation IDs: getHealth, createSession, getSession, deleteSession, listSeats, listAccounts, listBindings, getCsvImport, createCsvImport, commitCsvImport, deleteCsvImport, getProvisioningWindow, updateProvisioningWindow, listEnrollmentReviews, approveEnrollmentReview, denyEnrollmentReview, listDevices, getDevice, updateDevice, deleteDeviceBinding, getDeviceSessionControl, setDeviceSessionLock, terminateDeviceSession, getDeviceHome, resetDeviceHome, getDeviceConvergence.\nDeclared but not mounted in WP8 operation IDs: none.";
const SESSION_COOKIE_SECURITY_SCHEME: &str = "sessionCookie";
const SESSION_COOKIE_NAME: &str = "__Secure-natsume_session";
const CANONICAL_UUID_V7_PATTERN: &str =
    "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";
const CANONICAL_UUID_V5_PATTERN: &str =
    "^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        crate::http::handler::health::health,
        crate::http::handler::session::create_session,
        crate::http::handler::session::read_session,
        crate::http::handler::session::delete_session,
        crate::http::handler::contest::seat::list_seats,
        crate::http::handler::contest::account::list_accounts,
        crate::http::handler::contest::binding::list_bindings,
        crate::http::handler::import::get_import,
        crate::http::handler::import::create_import,
        crate::http::handler::import::commit_import,
        crate::http::handler::import::delete_import,
        crate::http::handler::provisioning::get_provisioning_window,
        crate::http::handler::provisioning::update_provisioning_window,
        crate::http::handler::enrollment::list_enrollment_reviews,
        crate::http::handler::enrollment::approve_enrollment_review,
        crate::http::handler::enrollment::deny_enrollment_review,
        crate::http::handler::device::lifecycle::list_devices,
        crate::http::handler::device::lifecycle::get_device,
        crate::http::handler::device::lifecycle::update_device,
        crate::http::handler::device::binding::delete_device_binding,
        crate::http::handler::device::session::get_session_control,
        crate::http::handler::device::session::set_session_lock,
        crate::http::handler::device::session::terminate_session,
        crate::http::handler::device::home::get_home,
        crate::http::handler::device::home::reset_home,
        crate::http::handler::device::convergence::get_device_convergence
    ),
    components(schemas(
        crate::http::handler::health::HealthResponse,
        crate::http::handler::session::SessionRequest,
        crate::http::handler::session::SessionResponse,
        crate::http::handler::contest::seat::SeatResponse,
        crate::http::handler::contest::account::AccountResponse,
        crate::http::handler::contest::binding::BindingResponse,
        crate::http::handler::import::ImportMappingChangeResponse,
        crate::http::handler::import::ImportBindingImpactResponse,
        crate::http::handler::import::ImportRedactedDiff,
        crate::http::handler::import::ImportPreviewResponse,
        crate::http::handler::import::ImportPendingSummary,
        crate::http::handler::import::ImportPendingResponse,
        crate::http::handler::import::ImportCommitRequest,
        crate::http::handler::provisioning::ProvisioningWindowResponse,
        crate::http::handler::provisioning::ProvisioningWindowRequest,
        crate::http::handler::enrollment::EnrollmentReviewResponse,
        crate::http::handler::device::lifecycle::DeviceResponse,
        crate::http::handler::device::lifecycle::DeviceUpdateRequest,
        crate::http::handler::device::session::SessionControlResponse,
        crate::http::handler::device::session::SessionLockRequest,
        crate::http::handler::device::home::HomeResponse,
        crate::http::handler::device::convergence::DeviceConvergenceResponse
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
        "CanonicalUuidV7".to_owned(),
        canonical_uuid_v7_schema().into(),
    );
    components.schemas.insert(
        "CanonicalUuidV5".to_owned(),
        canonical_uuid_v5_schema().into(),
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
    for schema_name in ["DeviceResponse", "EnrollmentReviewResponse"] {
        if let Some(RefOr::T(Schema::Object(schema))) = components.schemas.get_mut(schema_name) {
            let property_name = if schema_name == "DeviceResponse" {
                "device_id"
            } else {
                "review_id"
            };
            schema.properties.insert(
                property_name.to_owned(),
                Ref::from_schema_name("CanonicalUuidV7").into(),
            );
            schema.properties.insert(
                "machine_hardware_id".to_owned(),
                Ref::from_schema_name("CanonicalUuidV5").into(),
            );
        }
    }
}

fn canonicalize_path_parameters(paths: &mut Paths) {
    for path in paths.paths.values_mut() {
        for operation in [
            path.get.as_mut(),
            path.post.as_mut(),
            path.put.as_mut(),
            path.delete.as_mut(),
            path.patch.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            let Some(parameters) = operation.parameters.as_mut() else {
                continue;
            };
            for parameter in parameters.iter_mut().filter(|parameter| {
                matches!(
                    parameter.name.as_str(),
                    "device_id" | "review_id" | "import_id"
                )
            }) {
                parameter.schema = Some(Ref::from_schema_name("CanonicalUuidV7").into());
            }
        }
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

fn canonical_uuid_v7_schema() -> ObjectBuilder {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid)))
        .pattern(Some(CANONICAL_UUID_V7_PATTERN))
}

fn canonical_uuid_v5_schema() -> ObjectBuilder {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid)))
        .pattern(Some(CANONICAL_UUID_V5_PATTERN))
}

fn error_response_schema() -> Schema {
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .property("title", ObjectBuilder::new().schema_type(Type::String))
        .property("status", ObjectBuilder::new().schema_type(Type::Integer))
        .property("code", ObjectBuilder::new().schema_type(Type::String))
        .required("title")
        .required("status")
        .required("code")
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
        enrich_operation(path_item.patch.as_mut());
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
