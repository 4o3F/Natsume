use std::collections::BTreeSet;

use natsume_server::db::MIGRATOR;
use sqlx::{Row, SqlitePool, query, query_as, query_scalar, sqlite::SqlitePoolOptions};

const APPLICATION_TABLES: &[&str] = &[
    "account_mappings",
    "accounts",
    "audit_events",
    "commands",
    "device_bindings",
    "device_tokens",
    "devices",
    "enrollment_requests",
    "gateway_certificates",
    "observed_device_states",
    "pending_import_candidate",
    "provisioning_window",
    "revision_counters",
    "seats",
    "server_vault_records",
    "site_identity",
];

const RETIRED_TABLES: &[&str] = &[
    "schema_metadata",
    "instance_state",
    "contest_configuration_state",
    "system_configuration_revisions",
    "automation_policy_revisions",
    "credential_revisions",
    "seat_account_mappings",
    "seat_assignments",
    "device_target_states",
    "csv_imports",
    "csv_import_rows",
    "operations",
    "operation_targets",
    "command_attempts",
    "idempotency_records",
    "change_events",
    "provisioning_window_revisions",
];

async fn migrated_pool() -> SqlitePool {
    let Ok(pool) = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
    else {
        panic!("in-memory SQLite must connect");
    };
    if let Err(error) = query("PRAGMA foreign_keys = ON").execute(&pool).await {
        panic!("foreign keys must be enabled for schema contracts: {error}");
    }
    if let Err(error) = MIGRATOR.run(&pool).await {
        panic!("embedded server migration must execute: {error}");
    }
    pool
}

async fn table_names(pool: &SqlitePool) -> BTreeSet<String> {
    let Ok(names) = query_scalar::<_, String>(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations' ORDER BY name",
    )
    .fetch_all(pool)
    .await
    else {
        panic!("application table names must be queryable");
    };
    names.into_iter().collect()
}

async fn column_names(pool: &SqlitePool, table: &str) -> BTreeSet<String> {
    let Ok(rows) = query("SELECT name FROM pragma_table_info(?)")
        .bind(table)
        .fetch_all(pool)
        .await
    else {
        panic!("columns for {table} must be queryable");
    };
    rows.into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect()
}

async fn index_names(pool: &SqlitePool, table: &str) -> BTreeSet<String> {
    let Ok(names) = query_scalar::<_, String>(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = ? AND sql IS NOT NULL ORDER BY name",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    else {
        panic!("indexes for {table} must be queryable");
    };
    names.into_iter().collect()
}

async fn foreign_key_targets(pool: &SqlitePool, table: &str) -> BTreeSet<String> {
    let Ok(rows) = query("SELECT \"table\" FROM pragma_foreign_key_list(?)")
        .bind(table)
        .fetch_all(pool)
        .await
    else {
        panic!("foreign keys for {table} must be queryable");
    };
    rows.into_iter()
        .map(|row| row.get::<String, _>("table"))
        .collect()
}

fn names(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[tokio::test]
async fn fresh_schema_contains_only_current_fact_and_security_ledger_tables() {
    let pool = migrated_pool().await;
    let actual = table_names(&pool).await;
    let expected = names(APPLICATION_TABLES);
    assert_eq!(actual, expected);

    for retired in RETIRED_TABLES {
        assert!(
            !actual.contains(*retired),
            "retired table {retired} returned"
        );
    }

    let Ok(trigger_count) =
        query_scalar::<_, i64>("SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger'")
            .fetch_one(&pool)
            .await
    else {
        panic!("trigger count must be queryable");
    };
    assert_eq!(trigger_count, 0);

    let Ok(strict_tables) = query_scalar::<_, String>(
        "SELECT name FROM pragma_table_list WHERE schema = 'main' AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations' AND strict = 1 ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    else {
        panic!("STRICT table metadata must be queryable");
    };
    assert_eq!(strict_tables.into_iter().collect::<BTreeSet<_>>(), expected);
}

#[tokio::test]
async fn revision_and_provisioning_singletons_start_from_closed_zero_state() {
    let pool = migrated_pool().await;
    let Ok((configuration_revision, binding_revision)) = query_as::<_, (i64, i64)>(
        "SELECT configuration_revision, binding_revision FROM revision_counters WHERE singleton = 1",
    )
    .fetch_one(&pool)
    .await
    else {
        panic!("revision-counter singleton must be seeded");
    };
    assert_eq!((configuration_revision, binding_revision), (0, 0));

    let Ok((state, revision, last_audit_event_id)) = query_as::<_, (String, i64, Option<String>)>(
        "SELECT state, revision, last_audit_event_id FROM provisioning_window WHERE singleton = 1",
    )
    .fetch_one(&pool)
    .await
    else {
        panic!("provisioning-window singleton must be seeded");
    };
    assert_eq!(state, "closed");
    assert_eq!(revision, 0);
    assert!(last_audit_event_id.is_none());

    let Ok((revision_rows, provisioning_rows)) = query_as::<_, (i64, i64)>(
        "SELECT (SELECT COUNT(*) FROM revision_counters), (SELECT COUNT(*) FROM provisioning_window)",
    )
    .fetch_one(&pool)
    .await
    else {
        panic!("singleton row counts must be queryable");
    };
    assert_eq!((revision_rows, provisioning_rows), (1, 1));
}

#[tokio::test]
async fn current_credentials_mappings_bindings_and_import_candidate_have_single_current_rows() {
    let pool = migrated_pool().await;
    let statements = [
        "INSERT INTO server_vault_records(vault_record_id, record_type, subject_id, nonce, ciphertext) VALUES ('vault-account-1', 'account_password', 'account-1', x'01', x'02')",
        "INSERT INTO server_vault_records(vault_record_id, record_type, subject_id, nonce, ciphertext) VALUES ('vault-import-1', 'pending_import', 'candidate-1', x'03', x'04')",
        "INSERT INTO server_vault_records(vault_record_id, record_type, subject_id, nonce, ciphertext) VALUES ('vault-import-2', 'pending_import', 'candidate-2', x'05', x'06')",
        "INSERT INTO accounts(account_id, domjudge_username, credential_vault_record_id, credential_revision) VALUES ('account-1', 'team001', 'vault-account-1', 1)",
        "INSERT INTO seats(seat_id, seat_code) VALUES ('seat-1', 'A-01')",
        "INSERT INTO devices(device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES ('device-1', 'machine-1', 'strong', 'enrolled')",
        "INSERT INTO account_mappings(seat_id, account_id) VALUES ('seat-1', 'account-1')",
        "INSERT INTO device_bindings(seat_id, device_pk, binding_revision) VALUES ('seat-1', 'device-1', 1)",
        "INSERT INTO pending_import_candidate(singleton, candidate_id, expires_at, baseline_configuration_revision, baseline_binding_revision, preview_token_hash, payload_vault_record_id, redacted_preview_json) VALUES (1, 'candidate-1', '2026-08-03T01:00:00Z', 0, 0, zeroblob(32), 'vault-import-1', '{}')",
    ];
    for statement in statements {
        if let Err(error) = query(statement).execute(&pool).await {
            panic!("current-fact fixture must insert: {error}");
        }
    }

    let duplicate_vault = "INSERT INTO server_vault_records(vault_record_id, record_type, subject_id, nonce, ciphertext) VALUES ('vault-account-2', 'account_password', 'account-1', x'07', x'08')";
    assert!(query(duplicate_vault).execute(&pool).await.is_err());

    let second_candidate = "INSERT INTO pending_import_candidate(singleton, candidate_id, expires_at, baseline_configuration_revision, baseline_binding_revision, preview_token_hash, payload_vault_record_id, redacted_preview_json) VALUES (2, 'candidate-2', '2026-08-03T01:00:00Z', 0, 0, randomblob(32), 'vault-import-2', '{}')";
    assert!(query(second_candidate).execute(&pool).await.is_err());

    assert_eq!(
        column_names(&pool, "server_vault_records").await,
        names(&[
            "ciphertext",
            "nonce",
            "record_type",
            "subject_id",
            "vault_record_id",
        ])
    );
    assert_eq!(
        column_names(&pool, "accounts").await,
        names(&[
            "account_id",
            "credential_revision",
            "credential_vault_record_id",
            "domjudge_username",
        ])
    );
    assert_eq!(
        column_names(&pool, "account_mappings").await,
        names(&["account_id", "seat_id"])
    );
    assert_eq!(
        column_names(&pool, "device_bindings").await,
        names(&["binding_revision", "device_pk", "seat_id"])
    );
    assert_eq!(
        column_names(&pool, "pending_import_candidate").await,
        names(&[
            "baseline_binding_revision",
            "baseline_configuration_revision",
            "candidate_id",
            "expires_at",
            "payload_vault_record_id",
            "preview_token_hash",
            "redacted_preview_json",
            "singleton",
        ])
    );
}

#[tokio::test]
async fn enrollment_credentials_keep_only_current_token_and_required_certificate_ledger_fields() {
    let pool = migrated_pool().await;
    assert_eq!(
        column_names(&pool, "devices").await,
        names(&[
            "device_pk",
            "hardware_identity_quality",
            "machine_hardware_id",
            "state",
        ])
    );
    assert_eq!(
        column_names(&pool, "device_tokens").await,
        names(&["device_pk", "enrollment_request_id", "token_hash"])
    );
    assert_eq!(
        column_names(&pool, "gateway_certificates").await,
        names(&[
            "certificate_id",
            "device_pk",
            "enrollment_request_id",
            "not_after",
            "serial",
            "spki_sha256",
            "status",
        ])
    );
    assert_eq!(
        column_names(&pool, "provisioning_window").await,
        names(&["last_audit_event_id", "revision", "singleton", "state"])
    );
}

#[tokio::test]
async fn commands_are_direct_device_resources_with_one_frozen_payload_and_no_dispatcher_history() {
    let pool = migrated_pool().await;
    let columns = column_names(&pool, "commands").await;
    for required in [
        "command_id",
        "device_pk",
        "kind",
        "state",
        "request_fingerprint_version",
        "request_fingerprint_sha256",
        "payload_version",
        "frozen_payload_json",
        "created_at",
        "deadline_at",
        "created_audit_event_id",
        "redacted_terminal_result_json",
    ] {
        assert!(columns.contains(required), "missing commands.{required}");
    }
    for forbidden in [
        "operation_id",
        "operation_target_id",
        "idempotency_key",
        "nonsecret_payload_json",
        "frozen_seat_id",
        "frozen_account_id",
        "frozen_configuration_revision",
        "frozen_assignment_revision",
        "frozen_binding_revision",
        "frozen_credential_revision",
        "received_at",
        "last_delivery_at",
        "delivery_attempt_count",
        "completed_at",
        "device_token",
        "gateway_certificate",
        "secret_payload",
    ] {
        assert!(
            !columns.contains(forbidden),
            "forbidden commands.{forbidden}"
        );
    }

    assert_eq!(
        foreign_key_targets(&pool, "commands").await,
        names(&["audit_events", "devices"])
    );
    let indexes = index_names(&pool, "commands").await;
    for required in [
        "commands_by_deadline_at",
        "commands_by_device_state_created_at",
        "commands_by_group_correlation_id_created_at",
    ] {
        assert!(
            indexes.contains(required),
            "missing Command index {required}"
        );
    }
}

#[tokio::test]
async fn audit_is_minimal_and_observed_state_uses_binding_revision() {
    let pool = migrated_pool().await;
    let audit_columns = column_names(&pool, "audit_events").await;
    assert_eq!(
        audit_columns,
        names(&[
            "action_kind",
            "actor",
            "audit_event_id",
            "correlation_id",
            "group_correlation_id",
            "occurred_at",
            "reason_code",
            "redacted_detail_json",
            "resource_id",
            "resource_type",
            "result",
        ])
    );
    for forbidden in [
        "configuration_revision",
        "assignment_revision",
        "binding_revision",
        "credential_revision",
        "provisioning_revision",
        "target_count",
        "evidence_locator",
        "detail_json",
    ] {
        assert!(!audit_columns.contains(forbidden));
    }

    let observed_columns = column_names(&pool, "observed_device_states").await;
    for required in [
        "installed_binding_revision",
        "installed_credential_revision",
        "gateway_configuration_revision",
    ] {
        assert!(
            observed_columns.contains(required),
            "missing observed_device_states.{required}"
        );
    }
    for retired in [
        "installed_assignment_revision",
        "installed_credential_revision_id",
        "gateway_configuration_revision_id",
    ] {
        assert!(!observed_columns.contains(retired));
    }

    let audit_indexes = index_names(&pool, "audit_events").await;
    for required in [
        "audit_events_by_correlation_id_occurred_at",
        "audit_events_by_group_correlation_id_occurred_at",
        "audit_events_by_resource_type_resource_id_occurred_at",
    ] {
        assert!(
            audit_indexes.contains(required),
            "missing Audit index {required}"
        );
    }
}
