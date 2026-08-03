use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use natsume_error_code::ErrorCode;
use natsume_server::{
    db::{
        self,
        domain_checks::{
            DomainCheckError, EnrollmentIssuanceBinding, EnrollmentResolution, EnrollmentState,
            ProvisioningWindow, ProvisioningWindowState, WindowGatedOperation,
            check_device_token_insert, check_enrollment_insert, check_enrollment_issuance,
            check_gateway_certificate_insert,
        },
        guarded::{
            EnrollmentCredentials, GuardedWriteError, ProvisioningWindowChange,
            change_provisioning_window, issue_enrollment_credentials,
            replace_enrollment_credentials,
        },
    },
    error_contract::domain_check_error_code,
};
use sqlx::{Sqlite, SqlitePool, Transaction};

const SCHEMA: &str = include_str!("../../server/migrations/0001_initial.sql");
const DOMAIN_CHECKS: &str = include_str!("../../server/src/db/domain_checks.rs");
const OPENAPI_SOURCE: &str = include_str!("../../server/src/openapi.rs");
const ENROLLMENT_PATH: &str = "/api/v2/enrollment-requests/{request_id}/actions/approve";

#[derive(Debug, PartialEq, Eq)]
struct ServerTruthCounts {
    devices: i64,
    enrollments: i64,
    tokens: i64,
    certificates: i64,
    vault_records: i64,
    audits: i64,
}

fn table_body(name: &str) -> &'static str {
    let marker = format!("CREATE TABLE {name} (");
    let Some(start) = SCHEMA.find(&marker) else {
        panic!("INV-CERT schema evidence table must exist: {name}");
    };
    let rest = &SCHEMA[start + marker.len()..];
    let Some(end) = rest.find("\n) STRICT;") else {
        panic!("INV-CERT schema evidence table must have a locatable body: {name}");
    };
    &rest[..end]
}

fn rust_braced_body<'a>(source: &'a str, marker: &str) -> &'a str {
    let Some(start) = source.find(marker) else {
        panic!("INV-CERT Rust evidence marker must exist: {marker}");
    };
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut body_start = None;
    for index in start..bytes.len() {
        match bytes[index] {
            b'{' => {
                depth += 1;
                body_start.get_or_insert(index + 1);
            }
            b'}' => {
                let Some(next_depth) = depth.checked_sub(1) else {
                    panic!("INV-CERT Rust evidence braces must remain balanced: {marker}");
                };
                depth = next_depth;
                if depth == 0 {
                    let Some(body_start) = body_start else {
                        panic!("INV-CERT Rust evidence body must start: {marker}");
                    };
                    return &source[body_start..index];
                }
            }
            _ => {}
        }
    }
    panic!("INV-CERT Rust evidence body must terminate: {marker}");
}

async fn migrated_database(label: &str) -> (SqlitePool, PathBuf) {
    let path =
        std::env::temp_dir().join(format!("natsume-{label}-{}.sqlite", uuid::Uuid::now_v7()));
    let database_url = format!("sqlite://{}?mode=rwc", path.display());
    let Ok(pool) = db::connect_and_migrate(&database_url).await else {
        panic!("INV-CERT migrated SQLite database must open: {label}");
    };
    (pool, path)
}

async fn reopen_database(path: &Path, context: &str) -> SqlitePool {
    let database_url = format!("sqlite://{}?mode=rw", path.display());
    let Ok(pool) = db::connect_and_migrate(&database_url).await else {
        panic!("INV-CERT persisted SQLite database must reopen: {context}");
    };
    pool
}

fn remove_sqlite_files(path: &Path) {
    for candidate in [
        path.to_path_buf(),
        PathBuf::from(format!("{}-shm", path.display())),
        PathBuf::from(format!("{}-wal", path.display())),
    ] {
        if candidate.exists()
            && let Err(error) = std::fs::remove_file(&candidate)
        {
            panic!(
                "INV-CERT temporary SQLite artifact must be removable ({}): {error}",
                candidate.display()
            );
        }
    }
}

async fn server_truth_counts(pool: &SqlitePool) -> ServerTruthCounts {
    let Ok((devices, enrollments, tokens, certificates, vault_records, audits)) =
        sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64)>(
            "SELECT (SELECT COUNT(*) FROM devices), (SELECT COUNT(*) FROM enrollment_requests), (SELECT COUNT(*) FROM device_tokens), (SELECT COUNT(*) FROM gateway_certificates), (SELECT COUNT(*) FROM server_vault_records), (SELECT COUNT(*) FROM audit_events)",
        )
        .fetch_one(pool)
        .await
    else {
        panic!("INV-CERT Server-truth row counts must be queryable");
    };
    ServerTruthCounts {
        devices,
        enrollments,
        tokens,
        certificates,
        vault_records,
        audits,
    }
}

fn window_state(value: &str) -> ProvisioningWindowState {
    match value {
        "closed" => ProvisioningWindowState::Closed,
        "open" => ProvisioningWindowState::Open,
        _ => panic!("INV-CERT persisted provisioning-window state must be closed or open"),
    }
}

async fn current_window(pool: &SqlitePool) -> ProvisioningWindow {
    let Ok((state, revision)) = sqlx::query_as::<_, (String, i64)>(
        "SELECT state, revision FROM provisioning_window WHERE singleton = 1",
    )
    .fetch_one(pool)
    .await
    else {
        panic!("INV-CERT provisioning-window singleton must be queryable");
    };
    ProvisioningWindow {
        state: window_state(&state),
        revision,
    }
}

async fn execute_statements(
    transaction: &mut Transaction<'_, Sqlite>,
    statements: &[&'static str],
    context: &str,
) {
    for statement in statements {
        if let Err(error) = sqlx::query(*statement).execute(&mut **transaction).await {
            panic!("INV-CERT {context}: {error}");
        }
    }
}

async fn change_window(
    pool: &SqlitePool,
    state: ProvisioningWindowState,
    audit_event_id: &str,
    correlation_id: &str,
    occurred_at: &str,
) -> ProvisioningWindow {
    let (from_state, to_state) = match state {
        ProvisioningWindowState::Closed => ("open", "closed"),
        ProvisioningWindowState::Open => ("closed", "open"),
    };
    let detail = format!(r#"{{"from_state":"{from_state}","to_state":"{to_state}"}}"#);
    let Ok(mut transaction) = pool.begin().await else {
        panic!("INV-CERT provisioning-window transaction must begin");
    };
    let change = ProvisioningWindowChange {
        state,
        changed_by: "operator-1",
        audit_event_id,
        correlation_id,
        reason_code: Some("operator_requested"),
        redacted_detail_json: &detail,
        occurred_at,
    };
    let Ok(window) = change_provisioning_window(&mut transaction, change).await else {
        panic!("INV-CERT exact audited provisioning-window transition must pass");
    };
    if let Err(error) = transaction.commit().await {
        panic!("INV-CERT provisioning-window transition must commit atomically: {error}");
    }
    window
}

fn assert_closed_refusal(result: Result<(), DomainCheckError>, operation: WindowGatedOperation) {
    let Err(error) = result else {
        panic!("INV-CERT-01 closed window must refuse {operation:?}");
    };
    assert_eq!(
        error,
        DomainCheckError::ProvisioningWindowClosed { operation }
    );
    assert_eq!(
        domain_check_error_code(&error),
        Some(ErrorCode::ProvisioningWindowClosed)
    );
    assert_eq!(ErrorCode::ProvisioningWindowClosed.http_status(), 409);
}

#[tokio::test]
async fn closed_window_refuses_every_issuance_write_and_preserves_server_truth() {
    let (pool, path) = migrated_database("closed-window").await;
    let before = server_truth_counts(&pool).await;
    let window = current_window(&pool).await;
    assert_eq!(
        window,
        ProvisioningWindow {
            state: ProvisioningWindowState::Closed,
            revision: 0,
        }
    );

    let spki = [1_u8; 32];
    let binding = EnrollmentIssuanceBinding {
        enrollment_request_id: "enrollment-refused",
        state: EnrollmentState::Issued,
        resolved_device_pk: Some("device-refused"),
        gateway_spki_sha256: &spki,
    };
    assert_closed_refusal(
        check_enrollment_insert(window.state),
        WindowGatedOperation::EnrollmentInsert,
    );
    assert_closed_refusal(
        check_enrollment_issuance(
            window.state,
            "enrollment-refused",
            "audit-refused",
            EnrollmentResolution::CreateDevice,
            "operator-1",
            None,
        ),
        WindowGatedOperation::EnrollmentIssuance,
    );
    assert_closed_refusal(
        check_device_token_insert(
            window.state,
            "device-refused",
            "enrollment-refused",
            Some(binding),
        ),
        WindowGatedOperation::DeviceTokenInsert,
    );
    assert_closed_refusal(
        check_gateway_certificate_insert(
            window.state,
            "device-refused",
            "enrollment-refused",
            &spki,
            Some(binding),
        ),
        WindowGatedOperation::GatewayCertificateInsert,
    );

    let token_hash = [0x11_u8; 32];
    let credentials = EnrollmentCredentials {
        enrollment_request_id: "enrollment-refused",
        device_pk: "device-refused",
        issuing_actor: "operator-1",
        audit_event_id: "audit-refused",
        occurred_at: "2026-08-03T00:00:00Z",
        correlation_id: "correlation-refused",
        reason_code: None,
        redacted_detail_json: "{}",
        token_hash: &token_hash,
        certificate_id: "refused-certificate",
        certificate_serial: "refused-serial",
        certificate_spki_sha256: &spki,
        certificate_not_after: "2026-09-01T00:00:00Z",
    };
    let Ok(mut transaction) = pool.begin().await else {
        panic!("INV-CERT guarded closed-window transaction must begin");
    };
    execute_statements(
        &mut transaction,
        &[
            "INSERT INTO devices(device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES ('device-refused', 'machine-refused', 'strong', 'enrolled')",
            "INSERT INTO enrollment_requests(enrollment_request_id, machine_hardware_id, hardware_identity_quality, gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, source_ip, state, created_at) VALUES ('enrollment-refused', 'machine-refused', 'strong', x'01', x'0101010101010101010101010101010101010101010101010101010101010101', 'test-client', 1, '192.0.2.1', 'approved', '2026-08-03T00:00:00Z')",
        ],
        "closed-window persisted issuance fixture must insert",
    )
    .await;
    let result = issue_enrollment_credentials(&mut transaction, credentials).await;
    assert!(matches!(
        result,
        Err(GuardedWriteError::DomainCheck {
            source: DomainCheckError::ProvisioningWindowClosed {
                operation: WindowGatedOperation::EnrollmentIssuance
            }
        })
    ));
    if let Err(error) = transaction.rollback().await {
        panic!("INV-CERT refused write transaction must roll back: {error}");
    }
    assert_eq!(server_truth_counts(&pool).await, before);

    pool.close().await;
    remove_sqlite_files(&path);
}

#[tokio::test]
async fn committed_provisioning_audit_cannot_be_replayed_for_a_new_transition() {
    let (pool, path) = migrated_database("window-audit-replay").await;
    change_window(
        &pool,
        ProvisioningWindowState::Open,
        "audit-window-open-replay",
        "correlation-window-open-replay",
        "2026-08-03T00:00:00Z",
    )
    .await;
    change_window(
        &pool,
        ProvisioningWindowState::Closed,
        "audit-window-close-replay",
        "correlation-window-close-replay",
        "2026-08-03T00:01:00Z",
    )
    .await;

    let Ok(mut transaction) = pool.begin().await else {
        panic!("INV-CERT replay transaction must begin");
    };
    let result = change_provisioning_window(
        &mut transaction,
        ProvisioningWindowChange {
            state: ProvisioningWindowState::Open,
            changed_by: "operator-1",
            audit_event_id: "audit-window-open-replay",
            correlation_id: "correlation-window-open-replay",
            reason_code: Some("operator_requested"),
            redacted_detail_json: r#"{"from_state":"closed","to_state":"open"}"#,
            occurred_at: "2026-08-03T00:00:00Z",
        },
    )
    .await;
    assert!(matches!(result, Err(GuardedWriteError::Database { .. })));
    if let Err(error) = transaction.rollback().await {
        panic!("INV-CERT replay transaction must roll back: {error}");
    }
    assert_eq!(
        current_window(&pool).await,
        ProvisioningWindow {
            state: ProvisioningWindowState::Closed,
            revision: 2,
        }
    );
    let Ok(audit_count) = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_events")
        .fetch_one(&pool)
        .await
    else {
        panic!("INV-CERT replay audit count must be queryable");
    };
    assert_eq!(audit_count, 2);

    pool.close().await;
    remove_sqlite_files(&path);
}

#[tokio::test]
async fn committed_enrollment_audit_cannot_be_replayed_for_credential_issuance() {
    let (pool, path) = migrated_database("enrollment-audit-replay").await;
    change_window(
        &pool,
        ProvisioningWindowState::Open,
        "audit-window-open-enrollment-replay",
        "correlation-window-open-enrollment-replay",
        "2026-08-03T00:00:00Z",
    )
    .await;
    for statement in [
        "INSERT INTO devices(device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES ('device-replay', 'machine-replay', 'strong', 'enrolled')",
        "INSERT INTO enrollment_requests(enrollment_request_id, machine_hardware_id, hardware_identity_quality, gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, source_ip, state, created_at) VALUES ('enrollment-replay', 'machine-replay', 'strong', x'01', x'3333333333333333333333333333333333333333333333333333333333333333', 'test-client', 1, '192.0.2.3', 'approved', '2026-08-03T00:01:00Z')",
        "INSERT INTO audit_events(audit_event_id, occurred_at, actor, action_kind, resource_type, resource_id, result, correlation_id, redacted_detail_json) VALUES ('audit-enrollment-replay', '2026-08-03T00:01:00Z', 'operator-1', 'issue_device_enrollment', 'enrollment_request', 'enrollment-replay', 'succeeded', 'correlation-enrollment-replay', '{}')",
    ] {
        if let Err(error) = sqlx::query(statement).execute(&pool).await {
            panic!("INV-CERT committed replay fixture must insert: {error}");
        }
    }

    let token_hash = [0x33_u8; 32];
    let spki = [0x33_u8; 32];
    let Ok(mut transaction) = pool.begin().await else {
        panic!("INV-CERT Enrollment replay transaction must begin");
    };
    let result = issue_enrollment_credentials(
        &mut transaction,
        EnrollmentCredentials {
            enrollment_request_id: "enrollment-replay",
            device_pk: "device-replay",
            issuing_actor: "operator-1",
            audit_event_id: "audit-enrollment-replay",
            occurred_at: "2026-08-03T00:01:00Z",
            correlation_id: "correlation-enrollment-replay",
            reason_code: None,
            redacted_detail_json: "{}",
            token_hash: &token_hash,
            certificate_id: "certificate-replay",
            certificate_serial: "serial-replay",
            certificate_spki_sha256: &spki,
            certificate_not_after: "2026-09-01T00:00:00Z",
        },
    )
    .await;
    assert!(matches!(result, Err(GuardedWriteError::Database { .. })));
    if let Err(error) = transaction.rollback().await {
        panic!("INV-CERT Enrollment replay transaction must roll back: {error}");
    }

    let Ok((state, resolution, token_count, certificate_count, audit_count)) =
        sqlx::query_as::<_, (String, Option<String>, i64, i64, i64)>(
            "SELECT state, resolution, (SELECT COUNT(*) FROM device_tokens WHERE device_pk = 'device-replay'), (SELECT COUNT(*) FROM gateway_certificates WHERE device_pk = 'device-replay'), (SELECT COUNT(*) FROM audit_events WHERE audit_event_id = 'audit-enrollment-replay') FROM enrollment_requests WHERE enrollment_request_id = 'enrollment-replay'",
        )
        .fetch_one(&pool)
        .await
    else {
        panic!("INV-CERT rejected Enrollment replay state must be queryable");
    };
    assert_eq!(state, "approved");
    assert!(resolution.is_none());
    assert_eq!((token_count, certificate_count, audit_count), (0, 0, 1));

    pool.close().await;
    remove_sqlite_files(&path);
}

struct InactiveDeviceCase<'a> {
    device_pk: &'a str,
    machine_id: &'a str,
    enrollment_id: &'a str,
    audit_id: &'a str,
    correlation_id: &'a str,
    state: &'a str,
    certificate_id: &'a str,
    certificate_serial: &'a str,
}

async fn assert_inactive_device_issuance_refused(pool: &SqlitePool, case: InactiveDeviceCase<'_>) {
    if let Err(error) = sqlx::query(
        "INSERT INTO devices(device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES (?, ?, 'strong', ?)",
    )
    .bind(case.device_pk)
    .bind(case.machine_id)
    .bind(case.state)
    .execute(pool)
    .await
    {
        panic!("INV-CERT inactive Device fixture must insert: {error}");
    }
    if let Err(error) = sqlx::query(
        "INSERT INTO enrollment_requests(enrollment_request_id, machine_hardware_id, hardware_identity_quality, gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, source_ip, state, created_at) VALUES (?, ?, 'strong', x'01', x'4444444444444444444444444444444444444444444444444444444444444444', 'test-client', 1, '192.0.2.4', 'approved', '2026-08-03T00:01:00Z')",
    )
    .bind(case.enrollment_id)
    .bind(case.machine_id)
    .execute(pool)
    .await
    {
        panic!("INV-CERT inactive Enrollment fixture must insert: {error}");
    }

    let token_hash = [0x44_u8; 32];
    let spki = [0x44_u8; 32];
    let Ok(mut transaction) = pool.begin().await else {
        panic!("INV-CERT inactive Device transaction must begin");
    };
    let result = issue_enrollment_credentials(
        &mut transaction,
        EnrollmentCredentials {
            enrollment_request_id: case.enrollment_id,
            device_pk: case.device_pk,
            issuing_actor: "operator-1",
            audit_event_id: case.audit_id,
            occurred_at: "2026-08-03T00:01:00Z",
            correlation_id: case.correlation_id,
            reason_code: None,
            redacted_detail_json: "{}",
            token_hash: &token_hash,
            certificate_id: case.certificate_id,
            certificate_serial: case.certificate_serial,
            certificate_spki_sha256: &spki,
            certificate_not_after: "2026-09-01T00:00:00Z",
        },
    )
    .await;
    if let Err(error) = transaction.rollback().await {
        panic!("INV-CERT inactive Device transaction must roll back: {error}");
    }
    assert!(
        matches!(
            result,
            Err(GuardedWriteError::DomainCheck {
                source: DomainCheckError::EnrollmentDeviceNotEnrolled
            })
        ),
        "{} Device must not receive credentials",
        case.state
    );
}

#[tokio::test]
async fn credential_issuance_rejects_revoked_and_disabled_devices() {
    let (pool, path) = migrated_database("inactive-device-issuance").await;
    change_window(
        &pool,
        ProvisioningWindowState::Open,
        "audit-window-open-inactive-device",
        "correlation-window-open-inactive-device",
        "2026-08-03T00:00:00Z",
    )
    .await;

    for case in [
        InactiveDeviceCase {
            device_pk: "device-revoked",
            machine_id: "machine-revoked",
            enrollment_id: "enrollment-revoked",
            audit_id: "audit-enrollment-revoked",
            correlation_id: "correlation-enrollment-revoked",
            state: "revoked",
            certificate_id: "certificate-revoked",
            certificate_serial: "serial-revoked",
        },
        InactiveDeviceCase {
            device_pk: "device-disabled",
            machine_id: "machine-disabled",
            enrollment_id: "enrollment-disabled",
            audit_id: "audit-enrollment-disabled",
            correlation_id: "correlation-enrollment-disabled",
            state: "disabled",
            certificate_id: "certificate-disabled",
            certificate_serial: "serial-disabled",
        },
    ] {
        assert_inactive_device_issuance_refused(&pool, case).await;
    }

    let Ok((issued, audits, tokens, certificates)) =
        sqlx::query_as::<_, (i64, i64, i64, i64)>(
            "SELECT (SELECT COUNT(*) FROM enrollment_requests WHERE state = 'issued'), (SELECT COUNT(*) FROM audit_events WHERE resource_type = 'enrollment_request'), (SELECT COUNT(*) FROM device_tokens), (SELECT COUNT(*) FROM gateway_certificates)",
        )
        .fetch_one(&pool)
        .await
    else {
        panic!("INV-CERT inactive Device refusal state must be queryable");
    };
    assert_eq!((issued, audits, tokens, certificates), (0, 0, 0, 0));

    pool.close().await;
    remove_sqlite_files(&path);
}

#[tokio::test]
async fn open_and_close_update_one_singleton_with_one_audit_each() {
    let (pool, path) = migrated_database("window-toggle").await;
    assert_eq!(
        change_window(
            &pool,
            ProvisioningWindowState::Open,
            "audit-window-open",
            "correlation-window-open",
            "2026-08-03T00:00:00Z",
        )
        .await,
        ProvisioningWindow {
            state: ProvisioningWindowState::Open,
            revision: 1,
        }
    );
    assert_eq!(
        change_window(
            &pool,
            ProvisioningWindowState::Closed,
            "audit-window-close",
            "correlation-window-close",
            "2026-08-03T00:01:00Z",
        )
        .await,
        ProvisioningWindow {
            state: ProvisioningWindowState::Closed,
            revision: 2,
        }
    );
    let Ok((window_count, audit_count, last_audit)) =
        sqlx::query_as::<_, (i64, i64, Option<String>)>(
            "SELECT (SELECT COUNT(*) FROM provisioning_window), (SELECT COUNT(*) FROM audit_events WHERE resource_type = 'provisioning_window'), last_audit_event_id FROM provisioning_window WHERE singleton = 1",
        )
        .fetch_one(&pool)
        .await
    else {
        panic!("INV-CERT singleton and audit counts must be queryable");
    };
    assert_eq!((window_count, audit_count), (1, 2));
    assert_eq!(last_audit.as_deref(), Some("audit-window-close"));

    pool.close().await;
    remove_sqlite_files(&path);
}

fn guarded_test_credentials<'a>(
    enrollment_request_id: &'a str,
    audit_event_id: &'a str,
    correlation_id: &'a str,
    token_hash: &'a [u8],
    certificate_id: &'a str,
    certificate_serial: &'a str,
    certificate_spki_sha256: &'a [u8],
) -> EnrollmentCredentials<'a> {
    EnrollmentCredentials {
        enrollment_request_id,
        device_pk: "device-1",
        issuing_actor: "operator-1",
        audit_event_id,
        occurred_at: "2026-08-03T00:10:00Z",
        correlation_id,
        reason_code: None,
        redacted_detail_json: "{}",
        token_hash,
        certificate_id,
        certificate_serial,
        certificate_spki_sha256,
        certificate_not_after: "2026-09-01T00:00:00Z",
    }
}

async fn seed_initial_issuance(pool: &SqlitePool) {
    let Ok(mut transaction) = pool.begin().await else {
        panic!("INV-CERT initial issuance transaction must begin");
    };
    execute_statements(
        &mut transaction,
        &[
            "INSERT INTO devices(device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES ('device-1', 'machine-1', 'strong', 'enrolled')",
            "INSERT INTO enrollment_requests(enrollment_request_id, machine_hardware_id, hardware_identity_quality, gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, source_ip, state, created_at) VALUES ('enrollment-1', 'machine-1', 'strong', x'01', x'1111111111111111111111111111111111111111111111111111111111111111', 'test-client', 1, '192.0.2.1', 'approved', '2026-08-03T00:01:00Z')",
        ],
        "initial Enrollment fixture must insert",
    )
    .await;
    let token_hash = [0xaa_u8; 32];
    let spki = [0x11_u8; 32];
    if let Err(error) = issue_enrollment_credentials(
        &mut transaction,
        guarded_test_credentials(
            "enrollment-1",
            "audit-enrollment-1",
            "correlation-enrollment-1",
            &token_hash,
            "certificate-1",
            "serial-1",
            &spki,
        ),
    )
    .await
    {
        panic!("INV-CERT initial credentials must insert atomically: {error}");
    }
    if let Err(error) = transaction.commit().await {
        panic!("INV-CERT initial issuance transaction must commit: {error}");
    }
}

async fn replace_credentials(pool: &SqlitePool) {
    let Ok(mut transaction) = pool.begin().await else {
        panic!("INV-CERT replacement transaction must begin");
    };
    execute_statements(
        &mut transaction,
        &[
            "INSERT INTO enrollment_requests(enrollment_request_id, machine_hardware_id, hardware_identity_quality, gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, source_ip, state, created_at) VALUES ('enrollment-2', 'machine-1', 'strong', x'02', x'2222222222222222222222222222222222222222222222222222222222222222', 'test-client', 1, '192.0.2.2', 'approved', '2026-08-03T00:02:00Z')",
        ],
        "replacement Enrollment fixture must insert",
    )
    .await;
    let token_hash = [0xbb_u8; 32];
    let spki = [0x22_u8; 32];
    if let Err(error) = replace_enrollment_credentials(
        &mut transaction,
        guarded_test_credentials(
            "enrollment-2",
            "audit-enrollment-2",
            "correlation-enrollment-2",
            &token_hash,
            "certificate-2",
            "serial-2",
            &spki,
        ),
    )
    .await
    {
        panic!("INV-CERT replacement must retire and insert atomically: {error}");
    }
    if let Err(error) = transaction.commit().await {
        panic!("INV-CERT replacement transaction must commit: {error}");
    }
}

#[tokio::test]
async fn replacement_rejects_revoked_device_without_retiring_active_credentials() {
    let (pool, path) = migrated_database("replacement-revoked-device").await;
    change_window(
        &pool,
        ProvisioningWindowState::Open,
        "audit-window-open-revoked-replacement",
        "correlation-window-open-revoked-replacement",
        "2026-08-03T00:00:00Z",
    )
    .await;
    seed_initial_issuance(&pool).await;
    if let Err(error) =
        sqlx::query("UPDATE devices SET state = 'revoked' WHERE device_pk = 'device-1'")
            .execute(&pool)
            .await
    {
        panic!("INV-CERT Device revocation fixture must update: {error}");
    }

    let Ok(mut transaction) = pool.begin().await else {
        panic!("INV-CERT revoked replacement transaction must begin");
    };
    execute_statements(
        &mut transaction,
        &[
            "INSERT INTO enrollment_requests(enrollment_request_id, machine_hardware_id, hardware_identity_quality, gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, source_ip, state, created_at) VALUES ('enrollment-2', 'machine-1', 'strong', x'02', x'2222222222222222222222222222222222222222222222222222222222222222', 'test-client', 1, '192.0.2.2', 'approved', '2026-08-03T00:02:00Z')",
        ],
        "revoked replacement Enrollment fixture must insert",
    )
    .await;
    let token_hash = [0xbb_u8; 32];
    let spki = [0x22_u8; 32];
    let result = replace_enrollment_credentials(
        &mut transaction,
        guarded_test_credentials(
            "enrollment-2",
            "audit-enrollment-2",
            "correlation-enrollment-2",
            &token_hash,
            "certificate-2",
            "serial-2",
            &spki,
        ),
    )
    .await;
    assert!(matches!(
        result,
        Err(GuardedWriteError::DomainCheck {
            source: DomainCheckError::EnrollmentDeviceNotEnrolled
        })
    ));
    if let Err(error) = transaction.rollback().await {
        panic!("INV-CERT revoked replacement transaction must roll back: {error}");
    }

    let Ok((old_token, old_status, replacement_audits, replacement_enrollments)) =
        sqlx::query_as::<_, (i64, String, i64, i64)>(
            "SELECT (SELECT COUNT(*) FROM device_tokens WHERE device_pk = 'device-1' AND token_hash = x'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'), (SELECT status FROM gateway_certificates WHERE certificate_id = 'certificate-1'), (SELECT COUNT(*) FROM audit_events WHERE audit_event_id = 'audit-enrollment-2'), (SELECT COUNT(*) FROM enrollment_requests WHERE enrollment_request_id = 'enrollment-2')",
        )
        .fetch_one(&pool)
        .await
    else {
        panic!("INV-CERT revoked replacement state must be queryable");
    };
    assert_eq!(old_token, 1);
    assert_eq!(old_status, "active");
    assert_eq!((replacement_audits, replacement_enrollments), (0, 0));

    pool.close().await;
    remove_sqlite_files(&path);
}

#[tokio::test]
async fn repeated_enrollment_rejects_a_reused_token_hash_without_retiring_credentials() {
    let (pool, path) = migrated_database("replacement-token-reuse").await;
    change_window(
        &pool,
        ProvisioningWindowState::Open,
        "audit-window-open-token-reuse",
        "correlation-window-open-token-reuse",
        "2026-08-03T00:00:00Z",
    )
    .await;
    seed_initial_issuance(&pool).await;

    let Ok(mut transaction) = pool.begin().await else {
        panic!("INV-CERT reused-token replacement transaction must begin");
    };
    execute_statements(
        &mut transaction,
        &[
            "INSERT INTO enrollment_requests(enrollment_request_id, machine_hardware_id, hardware_identity_quality, gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, source_ip, state, created_at) VALUES ('enrollment-2', 'machine-1', 'strong', x'02', x'2222222222222222222222222222222222222222222222222222222222222222', 'test-client', 1, '192.0.2.2', 'approved', '2026-08-03T00:02:00Z')",
        ],
        "reused-token replacement fixture must insert",
    )
    .await;
    let reused_token_hash = [0xaa_u8; 32];
    let replacement_spki = [0x22_u8; 32];
    let result = replace_enrollment_credentials(
        &mut transaction,
        guarded_test_credentials(
            "enrollment-2",
            "audit-enrollment-2",
            "correlation-enrollment-2",
            &reused_token_hash,
            "certificate-2",
            "serial-2",
            &replacement_spki,
        ),
    )
    .await;
    assert!(matches!(
        result,
        Err(GuardedWriteError::DomainCheck {
            source: DomainCheckError::ReplacementCredentialsRequired
        })
    ));

    let Ok((old_token, replacement_token, old_status, replacement_certificate)) =
        sqlx::query_as::<_, (i64, i64, String, i64)>(
            "SELECT (SELECT COUNT(*) FROM device_tokens WHERE device_pk = 'device-1' AND enrollment_request_id = 'enrollment-1' AND token_hash = x'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'), (SELECT COUNT(*) FROM device_tokens WHERE enrollment_request_id = 'enrollment-2'), (SELECT status FROM gateway_certificates WHERE certificate_id = 'certificate-1'), (SELECT COUNT(*) FROM gateway_certificates WHERE certificate_id = 'certificate-2')",
        )
        .fetch_one(&mut *transaction)
        .await
    else {
        panic!("INV-CERT rejected reused-token state must be queryable");
    };
    assert_eq!((old_token, replacement_token), (1, 0));
    assert_eq!(old_status, "active");
    assert_eq!(replacement_certificate, 0);

    if let Err(error) = transaction.rollback().await {
        panic!("INV-CERT rejected reused-token transaction must roll back: {error}");
    }
    let Ok((replacement_audits, replacement_enrollments, active_tokens, active_certificates)) =
        sqlx::query_as::<_, (i64, i64, i64, i64)>(
            "SELECT (SELECT COUNT(*) FROM audit_events WHERE audit_event_id = 'audit-enrollment-2'), (SELECT COUNT(*) FROM enrollment_requests WHERE enrollment_request_id = 'enrollment-2'), (SELECT COUNT(*) FROM device_tokens WHERE device_pk = 'device-1'), (SELECT COUNT(*) FROM gateway_certificates WHERE device_pk = 'device-1' AND status = 'active')",
        )
        .fetch_one(&pool)
        .await
    else {
        panic!("INV-CERT rolled-back reused-token state must be queryable");
    };
    assert_eq!(
        (
            replacement_audits,
            replacement_enrollments,
            active_tokens,
            active_certificates,
        ),
        (0, 0, 1, 1)
    );

    pool.close().await;
    remove_sqlite_files(&path);
}

#[tokio::test]
async fn repeated_enrollment_is_an_audited_atomic_replacement() {
    let (pool, path) = migrated_database("replacement").await;
    change_window(
        &pool,
        ProvisioningWindowState::Open,
        "audit-window-open-replacement",
        "correlation-window-open-replacement",
        "2026-08-03T00:00:00Z",
    )
    .await;
    seed_initial_issuance(&pool).await;
    replace_credentials(&pool).await;

    let Ok((old_tokens, new_tokens, old_status, new_status, replacement_audits, enrollments, devices, tokens, active_certificates)) =
        sqlx::query_as::<_, (i64, i64, String, String, i64, i64, i64, i64, i64)>(
            "SELECT (SELECT COUNT(*) FROM device_tokens WHERE token_hash = x'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'), (SELECT COUNT(*) FROM device_tokens WHERE token_hash = x'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'), (SELECT status FROM gateway_certificates WHERE certificate_id = 'certificate-1'), (SELECT status FROM gateway_certificates WHERE certificate_id = 'certificate-2'), (SELECT COUNT(*) FROM audit_events WHERE audit_event_id = 'audit-enrollment-2' AND action_kind = 'replace_device_enrollment' AND result = 'succeeded'), (SELECT COUNT(*) FROM enrollment_requests WHERE machine_hardware_id = 'machine-1' AND state = 'issued'), (SELECT COUNT(*) FROM devices WHERE machine_hardware_id = 'machine-1'), (SELECT COUNT(*) FROM device_tokens WHERE device_pk = 'device-1'), (SELECT COUNT(*) FROM gateway_certificates WHERE device_pk = 'device-1' AND status = 'active')",
        )
        .fetch_one(&pool)
        .await
    else {
        panic!("INV-CERT replacement truth must be queryable");
    };
    assert_eq!((old_tokens, new_tokens), (0, 1));
    assert_eq!(
        (old_status.as_str(), new_status.as_str()),
        ("retired", "active")
    );
    assert_eq!(replacement_audits, 1);
    assert_eq!(
        (enrollments, devices, tokens, active_certificates),
        (2, 1, 1, 1)
    );

    pool.close().await;
    remove_sqlite_files(&path);
}

#[tokio::test]
async fn restart_and_file_restore_close_persisted_open_window_once() {
    let (pool, path) = migrated_database("window-recovery").await;
    assert_eq!(
        current_window(&pool).await,
        ProvisioningWindow {
            state: ProvisioningWindowState::Closed,
            revision: 0,
        }
    );
    change_window(
        &pool,
        ProvisioningWindowState::Open,
        "audit-window-open-recovery",
        "correlation-window-open-recovery",
        "2026-08-03T00:00:00Z",
    )
    .await;
    pool.close().await;

    let restore_path = std::env::temp_dir().join(format!(
        "natsume-window-restore-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    if let Err(error) = std::fs::copy(&path, &restore_path) {
        panic!("INV-CERT open-window SQLite backup copy must succeed: {error}");
    }

    let reopened = reopen_database(&path, "restart").await;
    assert_eq!(
        current_window(&reopened).await,
        ProvisioningWindow {
            state: ProvisioningWindowState::Closed,
            revision: 2,
        }
    );
    let Ok(restart_audits) = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM audit_events WHERE action_kind = 'recovery_close_provisioning_window' AND actor = 'system:recovery' AND result = 'succeeded'",
    )
    .fetch_one(&reopened)
    .await
    else {
        panic!("INV-CERT restart recovery audit must be queryable");
    };
    assert_eq!(restart_audits, 1);
    reopened.close().await;

    let reopened_again = reopen_database(&path, "repeated restart").await;
    assert_eq!(
        current_window(&reopened_again).await,
        ProvisioningWindow {
            state: ProvisioningWindowState::Closed,
            revision: 2,
        }
    );
    let Ok(repeated_audits) = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM audit_events WHERE actor = 'system:recovery'",
    )
    .fetch_one(&reopened_again)
    .await
    else {
        panic!("INV-CERT repeated recovery audit count must be queryable");
    };
    assert_eq!(repeated_audits, 1);
    reopened_again.close().await;

    let restored = reopen_database(&restore_path, "file-copy restore").await;
    assert_eq!(
        current_window(&restored).await,
        ProvisioningWindow {
            state: ProvisioningWindowState::Closed,
            revision: 2,
        }
    );
    let Ok(restore_audits) = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM audit_events WHERE actor = 'system:recovery'",
    )
    .fetch_one(&restored)
    .await
    else {
        panic!("INV-CERT restore recovery audit count must be queryable");
    };
    assert_eq!(restore_audits, 1);
    restored.close().await;

    remove_sqlite_files(&path);
    remove_sqlite_files(&restore_path);
}

fn collect_credential_schema_markers(
    root: &serde_json::Value,
    value: &serde_json::Value,
    visited_refs: &mut BTreeSet<String>,
    output: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, child) in object {
                let normalized = key.to_ascii_lowercase();
                if ["token", "certificate", "credential", "leaf", "chain"]
                    .iter()
                    .any(|marker| normalized.contains(marker))
                {
                    output.push(key.clone());
                }
                if key == "$ref" {
                    if let Some(reference) = child.as_str()
                        && visited_refs.insert(reference.to_owned())
                        && let Some(pointer) = reference.strip_prefix('#')
                        && let Some(target) = root.pointer(pointer)
                    {
                        collect_credential_schema_markers(root, target, visited_refs, output);
                    }
                } else {
                    collect_credential_schema_markers(root, child, visited_refs, output);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                collect_credential_schema_markers(root, child, visited_refs, output);
            }
        }
        _ => {}
    }
}

#[test]
fn openapi_allows_credential_outputs_only_on_enrollment() {
    let Ok(document) = serde_json::to_value(natsume_server::openapi::openapi()) else {
        panic!("INV-CERT Rust-owned OpenAPI document must serialize");
    };
    let Some(paths) = document.get("paths").and_then(serde_json::Value::as_object) else {
        panic!("INV-CERT OpenAPI document must expose paths");
    };
    assert!(paths.contains_key(ENROLLMENT_PATH));

    let mut enrollment_markers = Vec::new();
    for (path, item) in paths {
        let Some(methods) = item.as_object() else {
            panic!("INV-CERT OpenAPI path item must be an object: {path}");
        };
        for method in ["get", "post", "put", "patch", "delete"] {
            let Some(operation) = methods.get(method) else {
                continue;
            };
            let Some(responses) = operation.get("responses") else {
                panic!("INV-CERT OpenAPI operation must declare responses: {method} {path}");
            };
            let mut markers = Vec::new();
            collect_credential_schema_markers(
                &document,
                responses,
                &mut BTreeSet::new(),
                &mut markers,
            );
            if path == ENROLLMENT_PATH {
                enrollment_markers.extend(markers);
            } else {
                assert!(
                    markers.is_empty(),
                    "INV-CERT non-Enrollment response exposes credential output at {method} {path}: {markers:?}"
                );
            }
        }
    }
    assert!(!enrollment_markers.is_empty());
}

#[test]
fn phase0_route_table_mounts_only_health() {
    let router = rust_braced_body(OPENAPI_SOURCE, "pub fn router() -> Router {");
    assert_eq!(
        router.trim(),
        "Router::new().route(\"/api/v2/health\", get(get_health))"
    );
}

fn assert_binding_required(result: &Result<(), DomainCheckError>, operation: &str) {
    assert!(
        matches!(result, Err(DomainCheckError::IssuedEnrollmentRequired)),
        "INV-CERT {operation} must require an issued Enrollment binding"
    );
}

#[test]
fn ddl_and_domain_checks_have_no_non_enrollment_issuance_path() {
    let token = table_body("device_tokens");
    let certificate = table_body("gateway_certificates");
    let commands = table_body("commands");
    assert!(token.contains(
        "enrollment_request_id TEXT NOT NULL UNIQUE REFERENCES enrollment_requests(enrollment_request_id)"
    ));
    assert!(certificate.contains(
        "enrollment_request_id TEXT NOT NULL UNIQUE REFERENCES enrollment_requests(enrollment_request_id)"
    ));
    assert!(!commands.to_ascii_lowercase().contains("certificate"));
    assert!(!commands.to_ascii_lowercase().contains("token"));
    assert!(!certificate.contains("command_id"));

    let spki = [3_u8; 32];
    let pending = EnrollmentIssuanceBinding {
        enrollment_request_id: "enrollment-pending",
        state: EnrollmentState::Pending,
        resolved_device_pk: Some("device-1"),
        gateway_spki_sha256: &spki,
    };
    assert_binding_required(
        &check_device_token_insert(
            ProvisioningWindowState::Open,
            "device-1",
            "enrollment-pending",
            None,
        ),
        "Device Token insertion without a binding",
    );
    assert_binding_required(
        &check_gateway_certificate_insert(
            ProvisioningWindowState::Open,
            "device-1",
            "enrollment-pending",
            &spki,
            None,
        ),
        "Gateway certificate insertion without a binding",
    );
    assert_binding_required(
        &check_device_token_insert(
            ProvisioningWindowState::Open,
            "device-1",
            "enrollment-pending",
            Some(pending),
        ),
        "Device Token insertion with a non-issued binding",
    );
}

#[test]
fn csr_self_assertions_have_no_authorization_input_path() {
    let enrollment = table_body("enrollment_requests").to_ascii_lowercase();
    assert!(enrollment.contains("gateway_csr_der"));
    assert!(enrollment.contains("gateway_spki_sha256"));
    for forbidden in [
        "common_name",
        "san",
        "hostname",
        "profile",
        "eku",
        "validity",
        "not_after",
        "csr_san",
    ] {
        assert!(
            !enrollment.contains(forbidden),
            "INV-CERT CSR self-asserted authorization column must be absent: {forbidden}"
        );
    }

    let binding = rust_braced_body(DOMAIN_CHECKS, "pub struct EnrollmentIssuanceBinding<'a> {")
        .to_ascii_lowercase();
    assert!(binding.contains("enrollment_request_id"));
    assert!(binding.contains("state"));
    assert!(binding.contains("resolved_device_pk"));
    assert!(binding.contains("gateway_spki_sha256"));
    for forbidden in [
        "csr",
        "san",
        "common_name",
        "hostname",
        "profile",
        "eku",
        "validity",
    ] {
        assert!(
            !binding.contains(forbidden),
            "INV-CERT issuance binding must not accept CSR authorization input: {forbidden}"
        );
    }
}
