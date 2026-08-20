use axum::{
    Router,
    body::Body,
    http::{HeaderValue, Method, Request, StatusCode, header},
};
use diesel::{QueryableByName, RunQueryDsl, sql_types::BigInt};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    application::{
        operator::{OperatorRole, sign_in, tests::PasswordVerificationTestGuard},
    },
    audit::CorrelationId,
    db::{
        Database,
        tests::{test_data_version, test_observer},
    },
};

use super::super::super::{
    router,
    tests::{
        Captured, SupportFailure, TestDatabase, canonical_correlation_id, check_error_response,
        drive, header_text, normalized_error_response_body, seed_operator, unused_vault_master_key,
        unused_web_root,
    },
};
use super::COMMAND_REQUEST_BODY_LIMIT_BYTES;

const ADMIN_LOGIN: &str = "command-http-admin";
const VIEWER_LOGIN: &str = "command-http-viewer";
const PASSWORD: &str = "command-http-password-canary";
const COMMAND_ID: &str = "0190abcd-ef01-7abc-8def-0123456789ab";
const DEVICE_ID: &str = "01900000-0000-7000-8000-000000000302";
const DISABLED_DEVICE_ID: &str = "01900000-0000-7000-8000-000000000303";
const REVOKED_DEVICE_ID: &str = "01900000-0000-7000-8000-000000000304";
const UNKNOWN_DEVICE_ID: &str = "01900000-0000-7000-8000-000000000399";
const GROUP_CORRELATION_ID: &str = "550e8400-e29b-41d4-a716-446655440000";
const FROZEN_PAYLOAD: &str = r#"{"requested_lock_epoch":43,"target":{"session_epoch":42,"session_instance_id":"session-a"}}"#;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn first_put_then_disable_replay_and_conflict_preserve_the_command() -> Result<(), TestFailure>
{
    let fixture = CommandHttpFixture::new().await?;
    fixture
        .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
        .await?;
    let cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
    let application = fixture.router();

    let created = put_command(
        &application,
        Some(&cookie),
        COMMAND_ID,
        Body::from(command_body(DEVICE_ID, None, None)),
    )
    .await?;
    assert_empty_success(&created, StatusCode::CREATED)?;
    let created_row = command_row(&fixture.database.database, COMMAND_ID).await?;
    let created_audit = created_audit(&fixture.database.database, COMMAND_ID).await?;
    if created_row.device_pk != DEVICE_ID
        || created_row.kind != "lock_session"
        || created_row.state != "created"
        || created_row.request_fingerprint_version != 1
        || created_row.request_fingerprint_sha256.len() != 32
        || created_row.group_correlation_id.is_some()
        || created_row.payload_version != 1
        || created_row.frozen_payload_json != FROZEN_PAYLOAD
        || created_row.created_at.is_empty()
        || created_row.deadline_at.is_some()
        || created_row.terminal_error_code.is_some()
        || created_row.redacted_terminal_result_json.is_some()
        || created_audit.audit_event_id != created_row.created_audit_event_id
        || created_audit.actor != "operator:self"
        || created_audit.action_kind != "command_create"
        || created_audit.resource_type != "command"
        || created_audit.resource_id.as_deref() != Some(COMMAND_ID)
        || created_audit.result != "succeeded"
        || created_audit.reason_code.as_deref() != Some("operator_requested")
        || created_audit.group_correlation_id.is_some()
        || created_audit.redacted_detail_json
            != r#"{"kind":"lock_session","payload_version":1,"request_fingerprint_version":1}"#
    {
        return Err(TestFailure::CreatedPersistenceChanged);
    }
    let fingerprint_hex = hex::encode(&created_row.request_fingerprint_sha256);
    if created_audit
        .redacted_detail_json
        .contains(&fingerprint_hex)
        || created_audit
            .redacted_detail_json
            .contains("request_fingerprint_sha256")
        || created_audit.redacted_detail_json.contains(FROZEN_PAYLOAD)
    {
        return Err(TestFailure::FingerprintOrRequestEscaped);
    }
    let after_created = command_snapshot(&fixture.database.database).await?;
    let created_detail_bytes = i64::try_from(created_audit.redacted_detail_json.len())
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    if after_created
        != (CommandSnapshot {
            commands: 1,
            audits: 1,
            detail_bytes: created_detail_bytes,
        })
    {
        return Err(TestFailure::CreatedPersistenceChanged);
    }

    crate::db::device::tests::test_set_device_state(
        &fixture.database.database,
        DEVICE_ID,
        "disabled",
    )
    .await
    .map_err(|_| TestFailure::FixtureFailed)?;
    let mut observer =
        test_observer(&fixture.database.path).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let version_before_replay =
        test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;

    let replayed = put_command(
        &application,
        Some(&cookie),
        COMMAND_ID,
        Body::from(command_body(DEVICE_ID, None, None)),
    )
    .await?;
    assert_empty_success(&replayed, StatusCode::OK)?;
    let version_after_replay =
        test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    if command_snapshot(&fixture.database.database).await? != after_created
        || command_row(&fixture.database.database, COMMAND_ID).await? != created_row
        || version_after_replay != version_before_replay
    {
        return Err(TestFailure::ReplayWroteData);
    }

    let conflict = put_command(
        &application,
        Some(&cookie),
        COMMAND_ID,
        Body::from(command_body(DEVICE_ID, Some("operator_requested"), None)),
    )
    .await?;
    check_error_response(
        &conflict,
        StatusCode::CONFLICT,
        "Conflict",
        "COMMAND_REQUEST_CONFLICT",
    )?;
    if command_row(&fixture.database.database, COMMAND_ID).await? != created_row {
        return Err(TestFailure::ConflictMutatedCommand);
    }
    let conflict_audit = conflict_audit(&fixture.database.database, COMMAND_ID).await?;
    if conflict_audit.actor != "operator:self"
        || conflict_audit.action_kind != "command_create"
        || conflict_audit.resource_type != "command"
        || conflict_audit.resource_id.as_deref() != Some(COMMAND_ID)
        || conflict_audit.result != "rejected"
        || conflict_audit.reason_code.as_deref() != Some("COMMAND_REQUEST_CONFLICT")
        || conflict_audit.group_correlation_id.is_some()
        || conflict_audit.redacted_detail_json != r#"{"request_fingerprint_version":1}"#
        || conflict_audit
            .redacted_detail_json
            .contains(&fingerprint_hex)
        || conflict_audit.redacted_detail_json.contains("sha256")
        || conflict_audit
            .redacted_detail_json
            .contains("operator_requested")
        || conflict_audit.redacted_detail_json.contains(FROZEN_PAYLOAD)
    {
        return Err(TestFailure::ConflictAuditChanged);
    }
    let after_conflict = command_snapshot(&fixture.database.database).await?;
    let conflict_detail_bytes = i64::try_from(conflict_audit.redacted_detail_json.len())
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    if after_conflict.commands != 1
        || after_conflict.audits != 2
        || after_conflict.detail_bytes != after_created.detail_bytes + conflict_detail_bytes
    {
        return Err(TestFailure::ConflictAuditChanged);
    }
    Ok(())
}

#[tokio::test]
async fn command_id_is_validated_before_body_and_invalid_forms_write_nothing()
-> Result<(), TestFailure> {
    let fixture = CommandHttpFixture::new().await?;
    fixture
        .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
        .await?;
    let cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
    let application = fixture.router();
    let invalid_ids = [
        COMMAND_ID.to_ascii_uppercase(),
        "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        COMMAND_ID.replace('-', ""),
        format!("{COMMAND_ID}trailing"),
    ];
    let before = command_snapshot(&fixture.database.database).await?;

    for command_id in invalid_ids {
        let response = put_command(
            &application,
            Some(&cookie),
            &command_id,
            Body::from("{malformed-body"),
        )
        .await?;
        check_error_response(
            &response,
            StatusCode::BAD_REQUEST,
            "Bad Request",
            "COMMAND_ID_INVALID",
        )?;
    }
    if command_snapshot(&fixture.database.database).await? != before {
        return Err(TestFailure::RejectedBoundaryWroteData);
    }
    Ok(())
}

#[tokio::test]
async fn missing_and_non_enrolled_devices_and_closed_requests_are_zero_write_errors()
-> Result<(), TestFailure> {
    let fixture = CommandHttpFixture::new().await?;
    fixture
        .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
        .await?;
    let cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
    let application = fixture.router();
    let before = command_snapshot(&fixture.database.database).await?;
    let mut observer =
        test_observer(&fixture.database.path).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    let version_before =
        test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;

    let mut normalized_not_found = Vec::new();
    for device_id in [UNKNOWN_DEVICE_ID, DISABLED_DEVICE_ID, REVOKED_DEVICE_ID] {
        let response = put_command(
            &application,
            Some(&cookie),
            COMMAND_ID,
            Body::from(command_body(device_id, None, None)),
        )
        .await?;
        check_error_response(
            &response,
            StatusCode::NOT_FOUND,
            "Not Found",
            "RESOURCE_NOT_FOUND",
        )?;
        normalized_not_found.push(normalized_error_response_body(&response)?);
    }
    if normalized_not_found[1..]
        .iter()
        .any(|body| body != &normalized_not_found[0])
    {
        return Err(TestFailure::DeviceEligibilityResponseChanged);
    }

    for invalid_body in [
        format!(
            r#"{{"device_id":"{DEVICE_ID}","kind":"lock_session","payload_version":1,"payload":{{"target":{{"session_instance_id":"session-a","session_epoch":42}},"requested_lock_epoch":43}},"unknown":true}}"#
        ),
        format!(
            r#"{{"device_id":"{DEVICE_ID}","payload_version":1,"payload":{{"target":{{"session_instance_id":"session-a","session_epoch":42}},"requested_lock_epoch":43}}}}"#
        ),
        format!(
            r#"{{"device_id":"{DEVICE_ID}","kind":"outside_enum","payload_version":1,"payload":{{}}}}"#
        ),
        format!(
            r#"{{"device_id":"{DEVICE_ID}","device_id":"{DEVICE_ID}","kind":"lock_session","payload_version":1,"payload":{{"target":{{"session_instance_id":"session-a","session_epoch":42}},"requested_lock_epoch":43}}}}"#
        ),
    ] {
        let response = put_command(
            &application,
            Some(&cookie),
            COMMAND_ID,
            Body::from(invalid_body),
        )
        .await?;
        check_error_response(
            &response,
            StatusCode::BAD_REQUEST,
            "Bad Request",
            "INVALID_REQUEST",
        )?;
    }
    let version_after =
        test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
    if command_snapshot(&fixture.database.database).await? != before
        || version_after != version_before
    {
        return Err(TestFailure::RejectedBoundaryWroteData);
    }
    Ok(())
}

#[tokio::test]
async fn authentication_role_and_body_limit_precede_command_domain_writes()
-> Result<(), TestFailure> {
    let fixture = CommandHttpFixture::new().await?;
    fixture
        .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
        .await?;
    fixture
        .seed_operator(VIEWER_LOGIN, OperatorRole::Viewer)
        .await?;
    let admin_cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
    let viewer_cookie = fixture.session_cookie(VIEWER_LOGIN).await?;
    let application = fixture.router();
    let before = command_snapshot(&fixture.database.database).await?;

    let unauthenticated = put_command(
        &application,
        None,
        COMMAND_ID,
        Body::from(command_body(DEVICE_ID, None, None)),
    )
    .await?;
    check_error_response(
        &unauthenticated,
        StatusCode::UNAUTHORIZED,
        "Unauthorized",
        "AUTHENTICATION_FAILED",
    )?;

    let viewer = put_command(
        &application,
        Some(&viewer_cookie),
        COMMAND_ID,
        Body::from(command_body(DEVICE_ID, None, None)),
    )
    .await?;
    check_error_response(
        &viewer,
        StatusCode::FORBIDDEN,
        "Forbidden",
        "AUTHORIZATION_DENIED",
    )?;

    let mut oversized_request = command_request(
        Some(&admin_cookie),
        COMMAND_ID,
        Body::from(vec![b'!'; COMMAND_REQUEST_BODY_LIMIT_BYTES + 1]),
    )?;
    oversized_request.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&(COMMAND_REQUEST_BODY_LIMIT_BYTES + 1).to_string())
            .map_err(|_| TestFailure::RequestBuildFailed)?,
    );
    let oversized = drive(&application, oversized_request).await?;
    assert_transport_payload_too_large(&oversized)?;

    if command_snapshot(&fixture.database.database).await? != before {
        return Err(TestFailure::RejectedBoundaryWroteData);
    }
    Ok(())
}

#[tokio::test]
async fn group_correlation_id_is_persisted_and_audited_without_payload_echo()
-> Result<(), TestFailure> {
    let fixture = CommandHttpFixture::new().await?;
    fixture
        .seed_operator(ADMIN_LOGIN, OperatorRole::Admin)
        .await?;
    let cookie = fixture.session_cookie(ADMIN_LOGIN).await?;
    let application = fixture.router();
    let response = put_command(
        &application,
        Some(&cookie),
        COMMAND_ID,
        Body::from(command_body(DEVICE_ID, None, Some(GROUP_CORRELATION_ID))),
    )
    .await?;
    assert_empty_success(&response, StatusCode::CREATED)?;
    let row = command_row(&fixture.database.database, COMMAND_ID).await?;
    let audit = created_audit(&fixture.database.database, COMMAND_ID).await?;
    if row.group_correlation_id.as_deref() != Some(GROUP_CORRELATION_ID)
        || audit.group_correlation_id.as_deref() != Some(GROUP_CORRELATION_ID)
        || audit.redacted_detail_json.contains(GROUP_CORRELATION_ID)
        || audit.redacted_detail_json.contains(FROZEN_PAYLOAD)
    {
        return Err(TestFailure::CreatedPersistenceChanged);
    }
    Ok(())
}

struct CommandHttpFixture {
    database: TestDatabase,
}

impl CommandHttpFixture {
    async fn new() -> Result<Self, TestFailure> {
        let database = TestDatabase::new().await?;
        seed_device(&database.database).await?;
        Ok(Self { database })
    }

    fn router(&self) -> Router {
        router(
            self.database.database.clone(),
            unused_vault_master_key(),
            unused_web_root(),
        )
    }

    async fn seed_operator(&self, login_name: &str, role: OperatorRole) -> Result<(), TestFailure> {
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

async fn seed_device(database: &Database) -> Result<(), TestFailure> {
    database
        .test_write(|connection| {
            diesel::sql_query(
                "INSERT INTO devices (device_pk, machine_hardware_id, \
                 hardware_identity_quality, state) VALUES \
                 ('01900000-0000-7000-8000-000000000302', 'command-http-hardware', \
                  'strong', 'enrolled'), \
                 ('01900000-0000-7000-8000-000000000303', \
                  'command-http-disabled-hardware', 'strong', 'disabled'), \
                 ('01900000-0000-7000-8000-000000000304', \
                  'command-http-revoked-hardware', 'strong', 'revoked')",
            )
            .execute(connection)
        })
        .await
        .map_err(|_| TestFailure::FixtureFailed)?
        .map(|_| ())
        .map_err(|_| TestFailure::FixtureFailed)
}

fn command_body(
    device_id: &str,
    reason_code: Option<&str>,
    group_correlation_id: Option<&str>,
) -> String {
    let reason =
        reason_code.map_or_else(String::new, |value| format!(r#", "reason_code":"{value}""#));
    let group = group_correlation_id.map_or_else(String::new, |value| {
        format!(r#", "group_correlation_id":"{value}""#)
    });
    format!(
        r#"{{"device_id":"{device_id}","kind":"lock_session","payload_version":1,"payload":{{"target":{{"session_instance_id":"session-a","session_epoch":42}},"requested_lock_epoch":43}}{reason}{group}}}"#
    )
}

async fn put_command(
    application: &Router,
    cookie: Option<&str>,
    command_id: &str,
    body: Body,
) -> Result<Captured, TestFailure> {
    let request = command_request(cookie, command_id, body)?;
    drive(application, request).await.map_err(TestFailure::from)
}

fn command_request(
    cookie: Option<&str>,
    command_id: &str,
    body: Body,
) -> Result<Request<Body>, TestFailure> {
    let mut request = Request::builder()
        .method(Method::PUT)
        .uri(format!("/api/v2/commands/{command_id}"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    request
        .body(body)
        .map_err(|_| TestFailure::RequestBuildFailed)
}

fn assert_empty_success(
    response: &Captured,
    expected_status: StatusCode,
) -> Result<(), TestFailure> {
    if response.status != expected_status || !response.body.is_empty() {
        return Err(TestFailure::SuccessResponseChanged);
    }
    canonical_correlation_id(&response.headers)?;
    Ok(())
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

async fn command_snapshot(database: &Database) -> Result<CommandSnapshot, TestFailure> {
    database
        .test_read(|connection| {
            diesel::sql_query(
                "SELECT (SELECT COUNT(*) FROM commands) AS commands, \
                 (SELECT COUNT(*) FROM audit_events WHERE action_kind = 'command_create') \
                    AS audits, \
                 (SELECT COALESCE(SUM(length(redacted_detail_json)), 0) FROM audit_events \
                    WHERE action_kind = 'command_create') AS detail_bytes",
            )
            .get_result::<CommandSnapshot>(connection)
        })
        .await
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)
}

async fn command_row(database: &Database, command_id: &str) -> Result<CommandRow, TestFailure> {
    let command_id = command_id.to_owned();
    database
        .test_read(move |connection| {
            diesel::sql_query(
                "SELECT command_id, device_pk, kind, state, request_fingerprint_version, \
                 request_fingerprint_sha256, group_correlation_id, payload_version, \
                 frozen_payload_json, created_at, deadline_at, terminal_error_code, \
                 redacted_terminal_result_json, created_audit_event_id \
                 FROM commands WHERE command_id = ?",
            )
            .bind::<diesel::sql_types::Text, _>(command_id)
            .get_result::<CommandRow>(connection)
        })
        .await
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)
}

async fn created_audit(
    database: &Database,
    command_id: &str,
) -> Result<AuditEvidence, TestFailure> {
    command_audit(database, command_id, "succeeded").await
}

async fn conflict_audit(
    database: &Database,
    command_id: &str,
) -> Result<AuditEvidence, TestFailure> {
    command_audit(database, command_id, "rejected").await
}

async fn command_audit(
    database: &Database,
    command_id: &str,
    result: &str,
) -> Result<AuditEvidence, TestFailure> {
    let command_id = command_id.to_owned();
    let result = result.to_owned();
    database
        .test_read(move |connection| {
            diesel::sql_query(
                "SELECT audit_event_id, actor, action_kind, resource_type, resource_id, result, \
                 reason_code, group_correlation_id, redacted_detail_json FROM audit_events \
                 WHERE action_kind = 'command_create' AND resource_id = ? AND result = ?",
            )
            .bind::<diesel::sql_types::Text, _>(command_id)
            .bind::<diesel::sql_types::Text, _>(result)
            .get_result::<AuditEvidence>(connection)
        })
        .await
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)
}

#[derive(Debug, PartialEq, Eq, QueryableByName)]
struct CommandSnapshot {
    #[diesel(sql_type = BigInt)]
    commands: i64,
    #[diesel(sql_type = BigInt)]
    audits: i64,
    #[diesel(sql_type = BigInt)]
    detail_bytes: i64,
}

#[derive(Debug, PartialEq, Eq, QueryableByName)]
struct CommandRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    command_id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    device_pk: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    kind: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    state: String,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    request_fingerprint_version: i32,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    request_fingerprint_sha256: Vec<u8>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    group_correlation_id: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    payload_version: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    frozen_payload_json: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    created_at: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    deadline_at: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    terminal_error_code: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    redacted_terminal_result_json: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Text)]
    created_audit_event_id: String,
}

#[derive(QueryableByName)]
struct AuditEvidence {
    #[diesel(sql_type = diesel::sql_types::Text)]
    audit_event_id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    actor: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    action_kind: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    resource_type: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    resource_id: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Text)]
    result: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    reason_code: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    group_correlation_id: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Text)]
    redacted_detail_json: String,
}

#[derive(Debug, Snafu)]
enum TestFailure {
    #[snafu(display("a shared HTTP test helper failed"))]
    #[snafu(context(false))]
    Support { source: SupportFailure },
    #[snafu(display("the command HTTP fixture failed"))]
    FixtureFailed,
    #[snafu(display("the command request could not be built"))]
    RequestBuildFailed,
    #[snafu(display("the command success response changed"))]
    SuccessResponseChanged,
    #[snafu(display("the command creation persistence contract changed"))]
    CreatedPersistenceChanged,
    #[snafu(display("the command replay wrote data"))]
    ReplayWroteData,
    #[snafu(display("the command conflict mutated the original command"))]
    ConflictMutatedCommand,
    #[snafu(display("the command conflict audit changed"))]
    ConflictAuditChanged,
    #[snafu(display("a command fingerprint or request escaped into audit detail"))]
    FingerprintOrRequestEscaped,
    #[snafu(display("a rejected command boundary wrote data"))]
    RejectedBoundaryWroteData,
    #[snafu(display("missing and non-enrolled Device responses diverged"))]
    DeviceEligibilityResponseChanged,
    #[snafu(display("the command body limit changed"))]
    BodyLimitChanged,
    #[snafu(display("command database evidence could not be read"))]
    DatabaseEvidenceFailed,
}
