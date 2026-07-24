const SCHEMA: &str = include_str!("../../server/migrations/0001_initial.sql");

fn table_body(name: &str) -> &'static str {
    let marker = format!("CREATE TABLE {name} (");
    let Some(start) = SCHEMA.find(&marker) else {
        panic!("table must exist: {name}");
    };
    let rest = &SCHEMA[start + marker.len()..];
    let Some(end) = rest.find("\n) STRICT;") else {
        panic!("table must end: {name}");
    };
    &rest[..end]
}

#[test]
fn enrollment_table_is_device_identity_only() {
    let enrollment = table_body("enrollment_requests");

    assert!(enrollment.contains("device_csr_der"));
    assert!(enrollment.contains("device_spki_sha256"));
    assert!(!enrollment.contains("gateway_csr"));
    assert!(!enrollment.contains("gateway_spki"));
}

#[test]
fn automation_has_no_certificate_or_secret_side_effect_switches() {
    let policy = table_body("automation_policy_revisions");

    assert!(policy.contains("auto_approve_enrollment"));
    assert!(policy.contains("auto_sync_state_after_binding"));
    assert!(!policy.contains("auto_issue_device_certificate"));
    assert!(!policy.contains("auto_issue_gateway_certificate"));
    assert!(!policy.contains("auto_sync_secret"));
}

#[test]
fn gateway_certificate_request_is_bound_to_sync_state_identity() {
    let request = table_body("gateway_certificate_requests");

    for field in [
        "command_id",
        "device_pk",
        "target_generation",
        "configuration_revision_id",
        "csr_der",
        "spki_sha256",
        "request_nonce_sha256",
    ] {
        assert!(request.contains(field), "missing request field {field}");
    }

    assert!(SCHEMA.contains("issued_for_command_id TEXT NOT NULL REFERENCES commands(command_id)"));
    assert!(SCHEMA.contains("CREATE UNIQUE INDEX one_active_gateway_certificate"));
}

#[test]
fn observed_state_persists_session_agent_platform_facts() {
    let observed = table_body("observed_device_states");

    for field in [
        "session_agent_state",
        "graphical_session_type",
        "display_backend",
        "ui_presentation_state",
        "session_screen_kind",
        "notifications_available",
        "desktop_lock_supported",
        "desktop_unlock_supported",
        "session_agent_error_code",
    ] {
        assert!(observed.contains(field), "missing observed field {field}");
    }

    assert!(observed.contains("presented_unfocused"));
    assert!(!observed.contains("session_supervisor"));
}

async fn migrated_database() -> (sqlx::SqlitePool, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("natsume-phase0-{}.sqlite", uuid::Uuid::now_v7()));
    let database_url = format!("sqlite://{}?mode=rwc", path.display());
    let Ok(pool) = natsume_server::db::connect_and_migrate(&database_url).await else {
        panic!("embedded migrations must execute against a new SQLite database");
    };
    (pool, path)
}

fn assert_sync_state_trigger(error: sqlx::Error) {
    let sqlx::Error::Database(database_error) = error else {
        panic!("trigger rejection must be a SQLite database error");
    };
    assert!(
        database_error
            .message()
            .contains("gateway_certificate_request_requires_sync_state")
    );
}

#[tokio::test]
async fn embedded_migrations_execute_and_are_repeatable() {
    use sqlx::Row;

    let (pool, path) = migrated_database().await;
    if let Err(error) = natsume_server::db::MIGRATOR.run(&pool).await {
        panic!("second migration run must be idempotent: {error}");
    }

    let Ok(columns) = sqlx::query("PRAGMA table_info(enrollment_requests)")
        .fetch_all(&pool)
        .await
    else {
        panic!("migrated enrollment table must be queryable");
    };
    let names: Vec<String> = columns
        .iter()
        .map(|row| {
            let Ok(name) = row.try_get::<String, _>("name") else {
                panic!("PRAGMA table_info must expose column names");
            };
            name
        })
        .collect();
    assert!(names.iter().any(|name| name == "device_csr_der"));
    assert!(names.iter().any(|name| name == "device_spki_sha256"));
    assert!(!names.iter().any(|name| name.contains("gateway")));

    pool.close().await;
    if let Err(error) = std::fs::remove_file(path) {
        panic!("temporary SQLite database must be removable: {error}");
    }
}

#[tokio::test]
async fn gateway_request_requires_same_device_sync_state_command() {
    let (pool, path) = migrated_database().await;
    let fixtures = [
        "INSERT INTO system_configuration_revisions(configuration_revision_id, revision_no, domjudge_upstream_url, domjudge_upstream_host_header, client_origin_hostname, browser_start_path, domjudge_login_path, gateway_certificate_profile_id, browser_policy_revision, home_template_revision, created_by, created_at) VALUES ('config-1', 1, 'https://judge.invalid', 'judge.invalid', 'device.invalid', '/', '/login', 'profile-1', 'browser-1', 'home-1', 'test', '2026-07-24T00:00:00Z')",
        "INSERT INTO devices(device_pk, machine_hardware_id, hardware_identity_quality, enrollment_state, row_version) VALUES ('device-1', 'machine-1', 'strong', 'enrolled', 1)",
        "INSERT INTO devices(device_pk, machine_hardware_id, hardware_identity_quality, enrollment_state, row_version) VALUES ('device-2', 'machine-2', 'strong', 'enrolled', 1)",
        "INSERT INTO operations(operation_id, kind, state, actor, selection_digest, target_count, created_at) VALUES ('operation-1', 'test', 'running', 'test', x'00', 1, '2026-07-24T00:00:00Z')",
        "INSERT INTO operations(operation_id, kind, state, actor, selection_digest, target_count, created_at) VALUES ('operation-2', 'test', 'running', 'test', x'01', 1, '2026-07-24T00:00:00Z')",
        "INSERT INTO operations(operation_id, kind, state, actor, selection_digest, target_count, created_at) VALUES ('operation-3', 'test', 'running', 'test', x'02', 1, '2026-07-24T00:00:00Z')",
        "INSERT INTO operation_targets(operation_target_id, operation_id, device_pk, state) VALUES ('target-sync', 'operation-1', 'device-1', 'running')",
        "INSERT INTO operation_targets(operation_target_id, operation_id, device_pk, state) VALUES ('target-wrong-kind', 'operation-2', 'device-1', 'running')",
        "INSERT INTO operation_targets(operation_target_id, operation_id, device_pk, state) VALUES ('target-mismatch', 'operation-3', 'device-1', 'running')",
        "INSERT INTO commands(command_id, operation_target_id, device_pk, kind, state, payload_json, created_at, deadline_at) VALUES ('command-sync', 'target-sync', 'device-1', 'SYNC_STATE', 'running', '{}', '2026-07-24T00:00:00Z', '2026-07-24T01:00:00Z')",
        "INSERT INTO commands(command_id, operation_target_id, device_pk, kind, state, payload_json, created_at, deadline_at) VALUES ('command-wrong-kind', 'target-wrong-kind', 'device-1', 'SYNC_SECRET', 'running', '{}', '2026-07-24T00:00:00Z', '2026-07-24T01:00:00Z')",
        "INSERT INTO commands(command_id, operation_target_id, device_pk, kind, state, payload_json, created_at, deadline_at) VALUES ('command-mismatch', 'target-mismatch', 'device-1', 'SYNC_STATE', 'running', '{}', '2026-07-24T00:00:00Z', '2026-07-24T01:00:00Z')",
    ];
    for statement in fixtures {
        if let Err(error) = sqlx::query(statement).execute(&pool).await {
            panic!("SQLite authorization fixture must insert: {error}");
        }
    }

    let accepted = "INSERT INTO gateway_certificate_requests(gateway_certificate_request_id, command_id, device_pk, target_generation, configuration_revision_id, csr_der, spki_sha256, request_nonce_sha256, state, created_at) VALUES ('request-ok', 'command-sync', 'device-1', 1, 'config-1', x'01', x'02', x'03', 'pending', '2026-07-24T00:00:00Z')";
    if let Err(error) = sqlx::query(accepted).execute(&pool).await {
        panic!("same-device SYNC_STATE request must satisfy the SQL invariant: {error}");
    }

    let wrong_kind = "INSERT INTO gateway_certificate_requests(gateway_certificate_request_id, command_id, device_pk, target_generation, configuration_revision_id, csr_der, spki_sha256, request_nonce_sha256, state, created_at) VALUES ('request-wrong-kind', 'command-wrong-kind', 'device-1', 1, 'config-1', x'01', x'04', x'05', 'pending', '2026-07-24T00:00:00Z')";
    let Err(error) = sqlx::query(wrong_kind).execute(&pool).await else {
        panic!("non-SYNC_STATE request must be rejected");
    };
    assert_sync_state_trigger(error);

    let mismatched_device = "INSERT INTO gateway_certificate_requests(gateway_certificate_request_id, command_id, device_pk, target_generation, configuration_revision_id, csr_der, spki_sha256, request_nonce_sha256, state, created_at) VALUES ('request-mismatch', 'command-mismatch', 'device-2', 1, 'config-1', x'01', x'06', x'07', 'pending', '2026-07-24T00:00:00Z')";
    let Err(error) = sqlx::query(mismatched_device).execute(&pool).await else {
        panic!("cross-device SYNC_STATE request must be rejected");
    };
    assert_sync_state_trigger(error);

    pool.close().await;
    if let Err(error) = std::fs::remove_file(path) {
        panic!("temporary SQLite database must be removable: {error}");
    }
}
