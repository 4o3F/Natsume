use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use diesel::{
    Connection, QueryableByName, RunQueryDsl,
    connection::SimpleConnection,
    sql_types::{BigInt, Binary, Nullable, Text},
    sqlite::SqliteConnection,
};
use natsume_server::{config::ServerConfig, openapi, router};
use serde_json::Value;
use tempfile::{TempDir, tempdir};
use tower::ServiceExt;
use uuid::Uuid;

use natsume_integration_tests::harness::bootstrap_operator;

const SESSION_HASH: [u8; 32] = [0x31; 32];
const DEVICE_A: &str = "01900000-0000-7000-8000-000000000301";
const DEVICE_B: &str = "01900000-0000-7000-8000-000000000302";

struct TestDatabase {
    _directory: TempDir,
    path: PathBuf,
    config_path: PathBuf,
}

impl TestDatabase {
    async fn new() -> Self {
        let directory = require_ok(tempdir(), "INV-CERT-01 directory must be created");
        let path = directory.path().join("server.sqlite3");
        let key_directory = directory.path().join("keys");
        require_ok(
            fs::create_dir(&key_directory),
            "INV-CERT-01 key directory must be created",
        );
        require_ok(
            fs::set_permissions(&key_directory, fs::Permissions::from_mode(0o700)),
            "INV-CERT-01 key directory must be private",
        );
        let config_path = directory.path().join("server.toml");
        require_ok(
            fs::write(
                &config_path,
                format!(
                    "[listen]\nhttps = \"127.0.0.1:0\"\n\
                     [storage]\ndatabase = \"{}\"\nroot_key = \"{}\"\n\
                     [tls]\ncertificate = \"{}\"\nprivate_key = \"{}\"\n\
                     [site]\nconfig = \"{}\"\ncontrol_root = \"{}\"\nlocal_origin_root = \"{}\"\n",
                    path.display(),
                    key_directory.join("server-root.key").display(),
                    directory.path().join("unused-server.der").display(),
                    key_directory.join("unused-server-key.pk8").display(),
                    directory.path().join("unused-site.toml").display(),
                    directory.path().join("unused-control-root.der").display(),
                    directory.path().join("unused-origin-root.der").display(),
                ),
            ),
            "INV-CERT-01 server configuration must be written",
        );
        bootstrap_operator(
            env!("CARGO_BIN_EXE_server-bootstrap-driver"),
            &config_path,
            "inv-cert-bootstrap",
            "inv-cert-password",
        )
        .await;
        Self {
            _directory: directory,
            path,
            config_path,
        }
    }

    fn observer(&self) -> SqliteConnection {
        let mut connection = require_ok(
            self.path
                .to_str()
                .ok_or(())
                .and_then(|path| SqliteConnection::establish(path).map_err(|_| ())),
            "INV-CERT-01 database observer must connect",
        );
        require_ok(
            connection.batch_execute("PRAGMA foreign_keys = ON"),
            "INV-CERT-01 database observer must enforce foreign keys",
        );
        connection
    }

    fn config(&self) -> ServerConfig {
        require_ok(
            ServerConfig::load_from(&self.config_path),
            "INV-CERT-01 server configuration must load",
        )
    }
}

#[derive(QueryableByName)]
struct OperatorIdRow {
    #[diesel(sql_type = Text)]
    operator_id: String,
}

#[derive(QueryableByName)]
struct ColumnNameRow {
    #[diesel(sql_type = Nullable<Text>)]
    name: Option<String>,
}

#[derive(QueryableByName)]
struct CredentialCountsRow {
    #[diesel(sql_type = BigInt)]
    device_token_count: i64,
    #[diesel(sql_type = BigInt)]
    gateway_certificate_count: i64,
}

fn require_ok<T, E>(result: Result<T, E>, message: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            drop(error);
            panic!("{message}");
        }
    }
}

fn insert_operator_session(connection: &mut SqliteConnection) {
    require_ok(
        diesel::sql_query(
            "INSERT INTO operator_accounts (operator_id, login_name, role, password_hash) \
             VALUES (?, 'inv-cert-operator', 'admin', 'redacted-test-phc')",
        )
        .bind::<Text, _>(Uuid::now_v7().to_string())
        .execute(connection),
        "operator account fixture must insert",
    );
    let operator_id = require_ok(
        diesel::sql_query(
            "SELECT operator_id FROM operator_accounts WHERE login_name = 'inv-cert-operator'",
        )
        .get_result::<OperatorIdRow>(connection),
        "operator account fixture must be readable",
    )
    .operator_id;
    require_ok(
        diesel::sql_query(
            "INSERT INTO operator_sessions \
             (session_credential_hash, operator_id, expires_at) \
             VALUES (?, ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+8 hours'))",
        )
        .bind::<Binary, _>(SESSION_HASH.as_slice())
        .bind::<Text, _>(operator_id)
        .execute(connection),
        "operator session fixture must insert",
    );
}

fn insert_device(connection: &mut SqliteConnection, device_id: &str, hardware_id: &str) {
    require_ok(
        diesel::sql_query(
            "INSERT INTO devices \
             (device_pk, machine_hardware_id, hardware_identity_quality, state) \
             VALUES (?, ?, 'strong', 'enrolled')",
        )
        .bind::<Text, _>(device_id)
        .bind::<Text, _>(hardware_id)
        .execute(connection),
        "Device fixture must insert",
    );
}

fn insert_pending_enrollment(
    connection: &mut SqliteConnection,
    request_id: &str,
    hardware_id: &str,
    spki_byte: u8,
) {
    require_ok(
        diesel::sql_query(
            "INSERT INTO enrollment_requests \
             (enrollment_request_id, machine_hardware_id, hardware_identity_quality, \
              gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, \
              source_ip, state, created_at) \
             VALUES (?, ?, 'strong', x'01', ?, 'test-client', 1, '192.0.2.1', 'pending', \
                     '2026-08-08T00:00:00.000Z')",
        )
        .bind::<Text, _>(request_id)
        .bind::<Text, _>(hardware_id)
        .bind::<Binary, _>([spki_byte; 32].as_slice())
        .execute(connection),
        "Enrollment fixture must insert",
    );
}

fn assert_rejected(result: &diesel::QueryResult<usize>, message: &str) {
    assert!(result.is_err(), "{message}");
}

#[tokio::test]
async fn operator_session_grants_no_device_credential_or_wss_identity() {
    let fixture = TestDatabase::new().await;
    let mut connection = fixture.observer();
    insert_operator_session(&mut connection);

    let columns = require_ok(
        diesel::sql_query("SELECT name FROM pragma_table_xinfo('operator_sessions') ORDER BY cid")
            .load::<ColumnNameRow>(&mut connection),
        "operator session columns must be queryable",
    )
    .into_iter()
    .map(|row| require_ok(row.name.ok_or(()), "column must be text"))
    .collect::<Vec<_>>();
    assert_eq!(
        columns,
        ["session_credential_hash", "operator_id", "expires_at"]
    );

    let credential_rows = require_ok(
        diesel::sql_query(
            "SELECT (SELECT COUNT(*) FROM device_tokens) AS device_token_count, \
             (SELECT COUNT(*) FROM gateway_certificates) AS gateway_certificate_count",
        )
        .get_result::<CredentialCountsRow>(&mut connection),
        "Device credential counts must be queryable",
    );
    assert_eq!(
        (
            credential_rows.device_token_count,
            credential_rows.gateway_certificate_count
        ),
        (0, 0)
    );

    let document = require_ok(
        serde_json::to_value(openapi::document()),
        "OpenAPI document must serialize",
    );
    let session_properties = require_object(
        &document,
        "/components/schemas/SessionResponse/properties",
        "session response properties must exist",
    );
    assert_eq!(
        session_properties
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["operator_id", "role"]
    );
    assert_browser_document_scopes_device_credentials_to_enrollment(&document);
}

#[tokio::test]
async fn schema_enforces_device_issuance_identity_and_single_active_certificate() {
    let fixture = TestDatabase::new().await;
    let mut connection = fixture.observer();
    insert_device(&mut connection, DEVICE_A, "machine-a");
    insert_device(&mut connection, DEVICE_B, "machine-b");
    insert_pending_enrollment(&mut connection, "request-a", "machine-a", 0x41);
    insert_pending_enrollment(&mut connection, "request-b", "machine-a", 0x42);

    assert_rejected(
        &diesel::sql_query(
            "INSERT INTO device_tokens (device_pk, enrollment_request_id, token_hash) \
             VALUES ('01900000-0000-7000-8000-000000000399', 'request-a', ?)",
        )
        .bind::<Binary, _>([0x71_u8; 32].as_slice())
        .execute(&mut connection),
        "an orphan Device Token was accepted",
    );
    assert_rejected(
        &diesel::sql_query(
            "INSERT INTO gateway_certificates \
             (certificate_id, device_pk, enrollment_request_id, serial, spki_sha256, not_after, status) \
             VALUES ('orphan-cert', '01900000-0000-7000-8000-000000000399', \
                     'request-a', 'orphan-serial', ?, '2027-08-08T00:00:00.000Z', 'active')",
        )
        .bind::<Binary, _>([0x72_u8; 32].as_slice())
        .execute(&mut connection),
        "an orphan Gateway certificate was accepted",
    );
    assert_rejected(
        &diesel::sql_query(
            "INSERT INTO enrollment_requests \
             (enrollment_request_id, machine_hardware_id, hardware_identity_quality, \
              gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, source_ip, \
              state, resolved_device_pk, created_at) \
             VALUES ('mismatched-composite', 'machine-b', 'strong', x'01', ?, 'test-client', 1, \
                     '192.0.2.1', 'pending', ?, '2026-08-08T00:00:00.000Z')",
        )
        .bind::<Binary, _>([0x73_u8; 32].as_slice())
        .bind::<Text, _>(DEVICE_A)
        .execute(&mut connection),
        "a mismatched composite Device identity was accepted",
    );

    require_ok(
        diesel::sql_query(
            "INSERT INTO gateway_certificates \
             (certificate_id, device_pk, enrollment_request_id, serial, spki_sha256, not_after, status) \
             VALUES ('active-a', ?, 'request-a', 'serial-a', ?, \
                     '2027-08-08T00:00:00.000Z', 'active')",
        )
        .bind::<Text, _>(DEVICE_A)
        .bind::<Binary, _>([0x74_u8; 32].as_slice())
        .execute(&mut connection),
        "first active certificate must insert",
    );
    assert_rejected(
        &diesel::sql_query(
            "INSERT INTO gateway_certificates \
             (certificate_id, device_pk, enrollment_request_id, serial, spki_sha256, not_after, status) \
             VALUES ('active-b', ?, 'request-b', 'serial-b', ?, \
                     '2027-08-08T00:00:00.000Z', 'active')",
        )
        .bind::<Text, _>(DEVICE_A)
        .bind::<Binary, _>([0x75_u8; 32].as_slice())
        .execute(&mut connection),
        "a second active certificate was accepted",
    );
}

#[tokio::test]
async fn mounted_and_declared_only_route_sets_are_distinct_on_the_real_router() {
    let fixture = TestDatabase::new().await;
    let application = require_ok(
        router(
            fixture.config(),
            Path::new("/natsume-integration-test-unused-web-root"),
        )
        .await,
        "INV-CERT-01 router composition must open the bootstrapped database",
    );
    for (method, path, expected) in mounted_routes() {
        assert_eq!(drive(&application, method, path).await, expected, "{path}");
    }

    for (method, path) in [(
        Method::PUT,
        "/api/v2/commands/01900000-0000-7000-8000-000000000399",
    )] {
        assert_eq!(
            drive(&application, method, path).await,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }

    let document = require_ok(
        serde_json::to_value(openapi::document()),
        "OpenAPI document must serialize",
    );
    let paths = require_object(&document, "/paths", "OpenAPI paths must exist");
    let mut operation_ids = paths
        .values()
        .filter_map(Value::as_object)
        .flat_map(|path| path.values())
        .filter_map(|operation| operation.get("operationId"))
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    operation_ids.sort_unstable();
    assert_eq!(
        operation_ids,
        [
            "approveEnrollment",
            "closeProvisioningWindow",
            "commitCsvImport",
            "createCsvImport",
            "createEnrollmentRequest",
            "createSession",
            "deleteSession",
            "disableDevice",
            "discardCsvImport",
            "getCsvImport",
            "getHealth",
            "getProvisioningWindow",
            "getSession",
            "listAccounts",
            "listBindings",
            "listDevices",
            "listEnrollmentRequests",
            "listSeats",
            "openProvisioningWindow",
            "putCommand",
            "rejectEnrollment",
            "revokeDevice",
        ]
    );
    assert_eq!(
        document
            .pointer("/info/description")
            .and_then(Value::as_str),
        Some(
            "Mounted Stage 5B operation IDs: getHealth, createSession, getSession, deleteSession, listSeats, listAccounts, listDevices, listBindings, revokeDevice, disableDevice, getCsvImport, createCsvImport, commitCsvImport, discardCsvImport, getProvisioningWindow, openProvisioningWindow, closeProvisioningWindow, createEnrollmentRequest, listEnrollmentRequests, approveEnrollment, rejectEnrollment.\nDeclared but not mounted in Stage 5B operation IDs: putCommand."
        )
    );
}

fn mounted_routes() -> [(Method, &'static str, StatusCode); 21] {
    [
        (Method::GET, "/api/v2/health", StatusCode::OK),
        (Method::POST, "/api/v2/session", StatusCode::BAD_REQUEST),
        (Method::GET, "/api/v2/session", StatusCode::UNAUTHORIZED),
        (Method::DELETE, "/api/v2/session", StatusCode::NO_CONTENT),
        (Method::GET, "/api/v2/seats", StatusCode::UNAUTHORIZED),
        (Method::GET, "/api/v2/accounts", StatusCode::UNAUTHORIZED),
        (Method::GET, "/api/v2/devices", StatusCode::UNAUTHORIZED),
        (Method::GET, "/api/v2/bindings", StatusCode::UNAUTHORIZED),
        (Method::GET, "/api/v2/imports", StatusCode::UNAUTHORIZED),
        (
            Method::GET,
            "/api/v2/provisioning-window",
            StatusCode::UNAUTHORIZED,
        ),
        (
            Method::POST,
            "/api/v2/devices/01900000-0000-7000-8000-000000000399/actions/revoke",
            StatusCode::UNAUTHORIZED,
        ),
        (
            Method::POST,
            "/api/v2/devices/01900000-0000-7000-8000-000000000399/actions/disable",
            StatusCode::UNAUTHORIZED,
        ),
        (Method::POST, "/api/v2/imports", StatusCode::UNAUTHORIZED),
        (
            Method::POST,
            "/api/v2/imports/01900000-0000-7000-8000-000000000399/actions/commit",
            StatusCode::UNAUTHORIZED,
        ),
        (
            Method::POST,
            "/api/v2/imports/01900000-0000-7000-8000-000000000399/actions/discard",
            StatusCode::UNAUTHORIZED,
        ),
        (
            Method::POST,
            "/api/v2/provisioning-window/actions/open",
            StatusCode::UNAUTHORIZED,
        ),
        (
            Method::POST,
            "/api/v2/provisioning-window/actions/close",
            StatusCode::UNAUTHORIZED,
        ),
        (
            Method::POST,
            "/api/v2/enrollment-requests",
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::GET,
            "/api/v2/enrollment-requests",
            StatusCode::UNAUTHORIZED,
        ),
        (
            Method::POST,
            "/api/v2/enrollment-requests/01900000-0000-7000-8000-000000000399/actions/approve",
            StatusCode::UNAUTHORIZED,
        ),
        (
            Method::POST,
            "/api/v2/enrollment-requests/01900000-0000-7000-8000-000000000399/actions/reject",
            StatusCode::UNAUTHORIZED,
        ),
    ]
}

async fn drive(application: &Router, method: Method, path: &str) -> StatusCode {
    let mut request = Request::builder().method(method).uri(path);
    let body = if path == "/api/v2/session" {
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from("{}")
    } else {
        Body::empty()
    };
    let mut request = require_ok(request.body(body), "request must build");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([198, 51, 100, 19], 45_123))));
    let response = require_ok(
        application.clone().oneshot(request).await,
        "router must answer",
    );
    let status = response.status();
    let _body = require_ok(
        to_bytes(response.into_body(), 16 * 1024).await,
        "response body must be bounded",
    );
    status
}

fn require_object<'a>(
    document: &'a Value,
    pointer: &str,
    message: &str,
) -> &'a serde_json::Map<String, Value> {
    match document.pointer(pointer).and_then(Value::as_object) {
        Some(value) => value,
        None => panic!("{message}"),
    }
}

fn assert_browser_document_scopes_device_credentials_to_enrollment(document: &Value) {
    let intake = require_object(
        document,
        "/paths/~1api~1v2~1enrollment-requests/post",
        "Enrollment intake operation must exist",
    );
    assert!(intake.get("security").is_none());
    assert_eq!(
        document
            .pointer("/paths/~1api~1v2~1enrollment-requests/post/responses/201/content/application~1json/schema/$ref")
            .and_then(Value::as_str),
        Some("#/components/schemas/EnrollmentIssuedResponse")
    );
    let issued_properties = require_object(
        document,
        "/components/schemas/EnrollmentIssuedResponse/properties",
        "Enrollment issuance response must exist",
    );
    assert_eq!(
        issued_properties
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "device_id",
            "device_token",
            "enrollment_request_id",
            "gateway_chain_der",
            "gateway_leaf_der",
            "state",
        ]
    );

    let mut non_enrollment_document = document.clone();
    require_object_mut(
        &mut non_enrollment_document,
        "/paths",
        "OpenAPI paths must exist",
    )
    .remove("/api/v2/enrollment-requests");
    let schemas = require_object_mut(
        &mut non_enrollment_document,
        "/components/schemas",
        "OpenAPI schemas must exist",
    );
    schemas.remove("EnrollmentRequest");
    schemas.remove("EnrollmentIssuedResponse");
    let encoded = require_ok(
        serde_json::to_string(&non_enrollment_document),
        "OpenAPI document must encode",
    )
    .to_ascii_lowercase()
    .chars()
    .filter(char::is_ascii_alphanumeric)
    .collect::<String>();
    for forbidden in [
        "devicetoken",
        "gatewayprivatekey",
        "gatewaycsr",
        "gatewaycertificate",
        "certificatebody",
        "onetimecredential",
        "enrollmentcredential",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "browser OpenAPI exposed {forbidden}"
        );
    }
}

fn require_object_mut<'a>(
    document: &'a mut Value,
    pointer: &str,
    message: &str,
) -> &'a mut serde_json::Map<String, Value> {
    match document.pointer_mut(pointer).and_then(Value::as_object_mut) {
        Some(value) => value,
        None => panic!("{message}"),
    }
}
