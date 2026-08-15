use std::{collections::BTreeMap, path::PathBuf};

use diesel::{
    Connection, QueryableByName, RunQueryDsl,
    connection::SimpleConnection,
    sql_types::{BigInt, Integer, Nullable, Text},
    sqlite::SqliteConnection,
};
use natsume_server::{
    application::provisioning::{ProvisioningError, RecoveryOutcome, recover_on_startup},
    db::{Database, DatabaseConfig, DatabaseError},
};
use uuid::Uuid;

struct TestDatabase {
    path: PathBuf,
}

impl TestDatabase {
    fn new() -> Self {
        Self::with_label("schema-contract")
    }

    fn with_label(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!("natsume-{label}-{}.sqlite3", Uuid::now_v7())),
        }
    }

    fn config(&self) -> DatabaseConfig {
        DatabaseConfig::new(&self.path, true)
    }

    fn observer(&self) -> SqliteConnection {
        let mut connection = require_ok(
            self.path
                .to_str()
                .ok_or(())
                .and_then(|path| SqliteConnection::establish(path).map_err(|_| ())),
            "database observer must connect",
        );
        require_ok(
            connection.batch_execute("PRAGMA foreign_keys = ON"),
            "database observer must enforce foreign keys",
        );
        connection
    }

    async fn connect(&self) -> Database {
        require_ok(
            Database::connect_and_migrate(&self.config()).await,
            "database connection and migration must succeed",
        )
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.path.with_extension("sqlite3-shm"));
        let _ = std::fs::remove_file(self.path.with_extension("sqlite3-wal"));
    }
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

fn require_database_error(result: Result<Database, DatabaseError>, message: &str) -> DatabaseError {
    match result {
        Err(error) => error,
        Ok(database) => {
            drop(database);
            panic!("{message}");
        }
    }
}

fn assert_database_error_is_redacted(error: DatabaseError, canary: &str) {
    let display = error.to_string();
    let debug = format!("{error:?}");
    for forbidden in [
        canary,
        "SELECT",
        "INSERT",
        "CREATE TABLE",
        "provisioning_window",
    ] {
        assert!(!display.contains(forbidden));
        assert!(!debug.contains(forbidden));
    }
}

#[derive(QueryableByName)]
struct TextRow {
    #[diesel(sql_type = Text)]
    value: String,
}

#[derive(QueryableByName)]
struct IntegerRow {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

#[derive(QueryableByName)]
struct ForeignKeyRow {
    #[diesel(sql_type = Text)]
    source: String,
    #[diesel(sql_type = Text)]
    target_table: String,
    #[diesel(sql_type = Text)]
    target_column: String,
}

#[derive(QueryableByName)]
struct ForeignKeyGroupRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
    #[diesel(sql_type = BigInt)]
    seq: i64,
    #[diesel(sql_type = Text)]
    source: String,
    #[diesel(sql_type = Text)]
    target: String,
}

#[derive(QueryableByName)]
struct IndexPropertiesRow {
    #[diesel(sql_type = Integer)]
    is_unique: i32,
    #[diesel(sql_type = Integer)]
    is_partial: i32,
}

#[derive(QueryableByName)]
struct RevisionCountersRow {
    #[diesel(sql_type = BigInt)]
    singleton: i64,
    #[diesel(sql_type = BigInt)]
    configuration_revision: i64,
    #[diesel(sql_type = BigInt)]
    binding_revision: i64,
}

#[derive(QueryableByName)]
struct ProvisioningWindowRow {
    #[diesel(sql_type = BigInt)]
    singleton: i64,
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = BigInt)]
    revision: i64,
    #[diesel(sql_type = Nullable<Text>)]
    last_audit_event_id: Option<String>,
}

#[derive(QueryableByName)]
struct RecoveryAuditRow {
    #[diesel(sql_type = Text)]
    audit_event_id: String,
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Text)]
    action_kind: String,
    #[diesel(sql_type = Text)]
    result: String,
    #[diesel(sql_type = Nullable<Text>)]
    reason_code: Option<String>,
    #[diesel(sql_type = Text)]
    correlation_id: String,
    #[diesel(sql_type = Text)]
    redacted_detail_json: String,
}

fn assert_provisioning_error_is_redacted(error: ProvisioningError, canary: &str) {
    let display = error.to_string();
    let debug = format!("{error:?}");
    for forbidden in [
        canary,
        "SELECT",
        "INSERT",
        "CREATE TABLE",
        "provisioning_window",
    ] {
        assert!(!display.contains(forbidden));
        assert!(!debug.contains(forbidden));
    }
}

fn application_tables(database: &TestDatabase) -> Vec<String> {
    let mut connection = database.observer();
    require_ok(
        diesel::sql_query(
            "SELECT name AS value FROM pragma_table_list \
             WHERE schema = 'main' AND name NOT LIKE 'sqlite_%' \
             AND name <> '__diesel_schema_migrations' ORDER BY name",
        )
        .load::<TextRow>(&mut connection),
        "application tables must be queryable",
    )
    .into_iter()
    .map(|row| row.value)
    .collect()
}

fn columns(database: &TestDatabase, table: &str) -> Vec<String> {
    let mut connection = database.observer();
    require_ok(
        diesel::sql_query("SELECT name AS value FROM pragma_table_xinfo(?) ORDER BY cid")
            .bind::<Text, _>(table)
            .load::<TextRow>(&mut connection),
        "table columns must be queryable",
    )
    .into_iter()
    .map(|row| row.value)
    .collect()
}

fn foreign_keys(database: &TestDatabase, table: &str) -> Vec<(String, String, String)> {
    let mut connection = database.observer();
    require_ok(
        diesel::sql_query(
            "SELECT \"from\" AS source, \"table\" AS target_table, \"to\" AS target_column \
             FROM pragma_foreign_key_list(?) ORDER BY \"from\", \"table\", \"to\"",
        )
        .bind::<Text, _>(table)
        .load::<ForeignKeyRow>(&mut connection),
        "foreign keys must be queryable",
    )
    .into_iter()
    .map(|row| (row.source, row.target_table, row.target_column))
    .collect()
}

fn enrollment_device_foreign_key_groups(
    database: &TestDatabase,
) -> Vec<Vec<(i64, String, String)>> {
    let mut connection = database.observer();
    let rows = require_ok(
        diesel::sql_query(
            "SELECT id, seq, \"from\" AS source, \"to\" AS target \
             FROM pragma_foreign_key_list('enrollment_requests') \
             WHERE \"table\" = 'devices' ORDER BY id, seq",
        )
        .load::<ForeignKeyGroupRow>(&mut connection),
        "enrollment foreign-key groups must be queryable",
    );
    let mut groups: BTreeMap<i64, Vec<(i64, String, String)>> = BTreeMap::new();
    for row in rows {
        groups
            .entry(row.id)
            .or_default()
            .push((row.seq, row.source, row.target));
    }
    let mut groups: Vec<Vec<(i64, String, String)>> = groups.into_values().collect();
    groups.sort_by_key(Vec::len);
    groups
}

fn index_properties(database: &TestDatabase, table: &str, index: &str) -> (bool, bool) {
    let mut connection = database.observer();
    let row = require_ok(
        diesel::sql_query(
            "SELECT \"unique\" AS is_unique, partial AS is_partial \
             FROM pragma_index_list(?) WHERE name = ?",
        )
        .bind::<Text, _>(table)
        .bind::<Text, _>(index)
        .get_result::<IndexPropertiesRow>(&mut connection),
        "index properties must be queryable",
    );
    (row.is_unique == 1, row.is_partial == 1)
}

const EXPECTED_COLUMNS: &[(&str, &[&str])] = &[
    ("account_mappings", &["seat_id", "account_id"]),
    (
        "accounts",
        &[
            "account_id",
            "domjudge_username",
            "credential_vault_record_id",
            "credential_revision",
        ],
    ),
    (
        "audit_events",
        &[
            "audit_event_id",
            "occurred_at",
            "actor",
            "action_kind",
            "resource_type",
            "resource_id",
            "result",
            "reason_code",
            "correlation_id",
            "group_correlation_id",
            "redacted_detail_json",
        ],
    ),
    (
        "commands",
        &[
            "command_id",
            "device_pk",
            "kind",
            "state",
            "request_fingerprint_version",
            "request_fingerprint_sha256",
            "group_correlation_id",
            "payload_version",
            "frozen_payload_json",
            "created_at",
            "deadline_at",
            "terminal_error_code",
            "redacted_terminal_result_json",
            "created_audit_event_id",
        ],
    ),
    (
        "device_bindings",
        &["seat_id", "device_pk", "binding_revision"],
    ),
    (
        "device_tokens",
        &["device_pk", "enrollment_request_id", "token_hash"],
    ),
    (
        "devices",
        &[
            "device_pk",
            "machine_hardware_id",
            "hardware_identity_quality",
            "state",
        ],
    ),
    (
        "enrollment_requests",
        &[
            "enrollment_request_id",
            "machine_hardware_id",
            "hardware_identity_quality",
            "gateway_csr_der",
            "gateway_spki_sha256",
            "client_version",
            "protocol_version",
            "source_ip",
            "state",
            "resolution",
            "resolved_device_pk",
            "issuance_audit_event_id",
            "created_at",
        ],
    ),
    (
        "gateway_certificates",
        &[
            "certificate_id",
            "device_pk",
            "enrollment_request_id",
            "serial",
            "spki_sha256",
            "not_after",
            "status",
        ],
    ),
    (
        "observed_device_states",
        &[
            "device_pk",
            "observed_sequence",
            "boot_id",
            "received_generation",
            "applied_generation",
            "applied_hash",
            "state_apply_status",
            "state_error_code",
            "installed_binding_revision",
            "installed_credential_revision",
            "secret_state",
            "gateway_state",
            "gateway_configuration_revision",
            "gateway_certificate_fingerprint",
            "gateway_certificate_not_after",
            "session_state",
            "session_instance_id",
            "session_epoch",
            "session_lock_state",
            "session_lock_epoch",
            "active_lock_command_id",
            "session_agent_state",
            "graphical_session_type",
            "display_backend",
            "ui_presentation_state",
            "session_screen_kind",
            "notifications_available",
            "desktop_lock_supported",
            "desktop_unlock_supported",
            "session_agent_error_code",
            "home_state",
            "observed_at",
        ],
    ),
    (
        "operator_accounts",
        &["operator_id", "login_name", "role", "password_hash"],
    ),
    (
        "operator_sessions",
        &["session_credential_hash", "operator_id", "expires_at"],
    ),
    (
        "pending_import_candidate",
        &[
            "singleton",
            "candidate_id",
            "expires_at",
            "baseline_configuration_revision",
            "baseline_binding_revision",
            "preview_token_hash",
            "payload_vault_record_id",
            "redacted_preview_json",
        ],
    ),
    (
        "provisioning_window",
        &["singleton", "state", "revision", "last_audit_event_id"],
    ),
    (
        "revision_counters",
        &["singleton", "configuration_revision", "binding_revision"],
    ),
    ("seats", &["seat_id", "seat_code"]),
    (
        "server_vault_records",
        &[
            "vault_record_id",
            "record_type",
            "subject_id",
            "nonce",
            "ciphertext",
        ],
    ),
    ("site_identity", &["singleton", "fleet_namespace_uuid"]),
];

fn expected_columns() -> BTreeMap<&'static str, &'static [&'static str]> {
    EXPECTED_COLUMNS.iter().copied().collect()
}

fn expected_foreign_keys() -> BTreeMap<&'static str, Vec<(&'static str, &'static str, &'static str)>>
{
    BTreeMap::from([
        (
            "account_mappings",
            vec![
                ("account_id", "accounts", "account_id"),
                ("seat_id", "seats", "seat_id"),
            ],
        ),
        (
            "accounts",
            vec![(
                "credential_vault_record_id",
                "server_vault_records",
                "vault_record_id",
            )],
        ),
        (
            "commands",
            vec![
                ("created_audit_event_id", "audit_events", "audit_event_id"),
                ("device_pk", "devices", "device_pk"),
            ],
        ),
        (
            "device_bindings",
            vec![
                ("device_pk", "devices", "device_pk"),
                ("seat_id", "seats", "seat_id"),
            ],
        ),
        (
            "device_tokens",
            vec![
                ("device_pk", "devices", "device_pk"),
                (
                    "enrollment_request_id",
                    "enrollment_requests",
                    "enrollment_request_id",
                ),
            ],
        ),
        (
            "enrollment_requests",
            vec![
                ("issuance_audit_event_id", "audit_events", "audit_event_id"),
                ("machine_hardware_id", "devices", "machine_hardware_id"),
                // The standalone FK and the first column of the composite FK
                // each produce this row in pragma_foreign_key_list.
                ("resolved_device_pk", "devices", "device_pk"),
                ("resolved_device_pk", "devices", "device_pk"),
            ],
        ),
        (
            "gateway_certificates",
            vec![
                ("device_pk", "devices", "device_pk"),
                (
                    "enrollment_request_id",
                    "enrollment_requests",
                    "enrollment_request_id",
                ),
            ],
        ),
        (
            "observed_device_states",
            vec![("device_pk", "devices", "device_pk")],
        ),
        (
            "operator_sessions",
            vec![("operator_id", "operator_accounts", "operator_id")],
        ),
        (
            "pending_import_candidate",
            vec![(
                "payload_vault_record_id",
                "server_vault_records",
                "vault_record_id",
            )],
        ),
        (
            "provisioning_window",
            vec![("last_audit_event_id", "audit_events", "audit_event_id")],
        ),
    ])
}

fn insert_open_window_fixture(database: &TestDatabase, audit_id: Uuid) {
    let mut connection = database.observer();
    let audit_id = audit_id.to_string();
    require_ok(
        diesel::sql_query(
            "INSERT INTO audit_events (audit_event_id, occurred_at, actor, action_kind, \
             resource_type, resource_id, result, reason_code, correlation_id, group_correlation_id, \
             redacted_detail_json) VALUES (?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), \
             'operator:test', 'open_provisioning_window', \
             'provisioning_window', NULL, 'succeeded', NULL, ?, NULL, '{}')",
        )
        .bind::<Text, _>(&audit_id)
        .bind::<Text, _>(Uuid::now_v7().to_string())
        .execute(&mut connection),
        "opening audit fixture must insert",
    );
    require_ok(
        diesel::sql_query(
            "UPDATE provisioning_window SET state = 'open', revision = 1, \
             last_audit_event_id = ? WHERE singleton = 1 AND state = 'closed' AND revision = 0",
        )
        .bind::<Text, _>(&audit_id)
        .execute(&mut connection),
        "open-window fixture must update",
    );
}

fn assert_foreign_key_enforcement(database: &TestDatabase) {
    let mut connection = database.observer();
    let violating_insert = diesel::sql_query(
        "INSERT INTO account_mappings (account_id, seat_id) VALUES ('missing-account', 'missing-seat')",
    )
    .execute(&mut connection);
    assert!(violating_insert.is_err());
}

fn assert_rejected(database: &TestDatabase, statement: &'static str, contract: &'static str) {
    let mut connection = database.observer();
    let result = connection.batch_execute(statement);
    assert!(
        result.is_err(),
        "{contract} accepted an out-of-domain value"
    );
}

fn execute_fixture_statement(
    database: &TestDatabase,
    statement: &'static str,
    message: &'static str,
) {
    let mut connection = database.observer();
    require_ok(connection.batch_execute(statement), message);
}

fn seed_constraint_prerequisites(database: &TestDatabase) {
    let statements = [
        "INSERT INTO server_vault_records VALUES \
         ('vault-1', 'account', 'subject-1', x'01', x'02')",
        "INSERT INTO server_vault_records VALUES \
         ('vault-2', 'import', 'subject-2', x'01', x'02')",
        "INSERT INTO server_vault_records VALUES \
         ('vault-3', 'account', 'subject-3', x'01', x'02')",
        "INSERT INTO seats VALUES ('seat-1', 'S1')",
        "INSERT INTO devices VALUES ('device-1', 'hardware-1', 'strong', 'enrolled')",
        "INSERT INTO operator_accounts VALUES ('operator-1', 'admin-1', 'admin', 'work-factor-hash')",
        "INSERT INTO accounts VALUES ('account-1', 'user-1', 'vault-1', 1)",
        "INSERT INTO audit_events VALUES \
         ('window-audit', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'operator:test', \
          'open_provisioning_window', 'provisioning_window', NULL, 'succeeded', NULL, \
          'window-correlation', NULL, '{}')",
        "INSERT INTO audit_events VALUES \
         ('command-audit', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'operator:test', \
          'create_command', 'command', 'command-1', 'succeeded', NULL, \
          'command-correlation', NULL, '{}')",
        "INSERT INTO enrollment_requests VALUES \
         ('request-1', 'hardware-1', 'strong', x'01', zeroblob(32), 'client', 1, \
          '192.0.2.1', 'pending', NULL, NULL, NULL, \
          '2026-01-01T00:00:00.000Z')",
        "INSERT INTO observed_device_states VALUES \
         ('device-1', 1, 'boot-1', 1, 1, NULL, 'applied', NULL, NULL, NULL, \
          'installed', 'ready', NULL, NULL, NULL, 'active', NULL, NULL, NULL, NULL, NULL, \
          'absent', NULL, NULL, 'hidden', 'hidden', 0, 0, 0, NULL, 'ready', \
          '2026-01-01T00:00:00.000Z')",
    ];
    let mut connection = database.observer();
    for statement in statements {
        require_ok(
            connection.batch_execute(statement),
            "constraint prerequisite must insert",
        );
    }
}

fn assert_closed_enums(database: &TestDatabase) {
    assert_rejected(
        database,
        "INSERT INTO devices VALUES ('bad-state', 'hardware-2', 'strong', 'unknown')",
        "devices.state",
    );
    assert_rejected(
        database,
        "INSERT INTO devices VALUES ('bad-quality', 'hardware-3', 'unknown', 'enrolled')",
        "devices.hardware_identity_quality",
    );
    assert_rejected(
        database,
        "INSERT INTO audit_events VALUES \
         ('bad-result', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'operator:test', 'test', \
          'test', NULL, 'unknown', NULL, 'correlation', NULL, '{}')",
        "audit_events.result",
    );
    assert_rejected(
        database,
        "INSERT INTO gateway_certificates VALUES \
         ('bad-status', 'device-1', 'request-1', 'serial-1', zeroblob(32), \
          '2027-01-01T00:00:00.000Z', 'unknown')",
        "gateway_certificates.status",
    );
    assert_rejected(
        database,
        "INSERT INTO enrollment_requests VALUES \
         ('bad-state-request', 'hardware-2', 'strong', x'01', zeroblob(32), 'client', 1, \
          '192.0.2.1', 'unknown', NULL, NULL, NULL, '2026-01-01T00:00:00.000Z')",
        "enrollment_requests.state",
    );
    assert_rejected(
        database,
        "INSERT INTO enrollment_requests VALUES \
         ('bad-resolution', 'hardware-3', 'strong', x'01', zeroblob(32), 'client', 1, \
          '192.0.2.1', 'pending', 'unknown', NULL, NULL, '2026-01-01T00:00:00.000Z')",
        "enrollment_requests.resolution",
    );
    assert_rejected(
        database,
        "INSERT INTO operator_accounts VALUES \
         ('bad-role', 'operator-2', 'owner', 'work-factor-hash')",
        "operator_accounts.role",
    );
    assert_rejected(
        database,
        "INSERT INTO commands VALUES \
         ('bad-kind', 'device-1', 'unknown', 'queued', 1, zeroblob(32), \
          NULL, 1, '{}', '2026-01-01T00:00:00.000Z', '2026-01-02T00:00:00.000Z', \
          NULL, NULL, 'command-audit')",
        "commands.kind",
    );
    execute_fixture_statement(
        database,
        "UPDATE provisioning_window SET state = 'open', revision = 1, \
         last_audit_event_id = 'window-audit' WHERE singleton = 1",
        "valid open-window prerequisite must update",
    );
    assert_rejected(
        database,
        "UPDATE provisioning_window SET state = 'unknown' WHERE singleton = 1",
        "provisioning_window.state",
    );
}

#[allow(clippy::too_many_arguments)]
fn assert_observed_enum_insert_rejected(
    database: &TestDatabase,
    device_pk: &str,
    state_apply_status: &str,
    secret_state: &str,
    gateway_state: &str,
    session_state: &str,
    session_lock_state: &str,
    home_state: &str,
    contract: &'static str,
) {
    let mut connection = database.observer();
    let hardware_id = format!("hardware-{device_pk}");
    require_ok(
        diesel::sql_query("INSERT INTO devices VALUES (?, ?, 'strong', 'enrolled')")
            .bind::<Text, _>(device_pk)
            .bind::<Text, _>(&hardware_id)
            .execute(&mut connection),
        "observed enum prerequisite device must insert",
    );
    let result = diesel::sql_query(
        "INSERT INTO observed_device_states (device_pk, observed_sequence, boot_id, \
         received_generation, applied_generation, state_apply_status, secret_state, \
         gateway_state, session_state, session_lock_state, home_state, observed_at) \
         VALUES (?, 1, 'boot-invalid-enum', 1, 1, ?, ?, ?, ?, ?, ?, \
         '2026-01-01T00:00:00.000Z')",
    )
    .bind::<Text, _>(device_pk)
    .bind::<Text, _>(state_apply_status)
    .bind::<Text, _>(secret_state)
    .bind::<Text, _>(gateway_state)
    .bind::<Text, _>(session_state)
    .bind::<Text, _>(session_lock_state)
    .bind::<Text, _>(home_state)
    .execute(&mut connection);
    assert!(
        result.is_err(),
        "{contract} accepted an out-of-domain value"
    );
}

fn assert_observed_closed_enums(database: &TestDatabase) {
    assert_observed_enum_insert_rejected(
        database,
        "bad-state-apply-status",
        "unknown",
        "installed",
        "ready",
        "active",
        "none",
        "ready",
        "observed_device_states.state_apply_status",
    );
    assert_observed_enum_insert_rejected(
        database,
        "bad-secret-state",
        "applied",
        "unknown",
        "ready",
        "active",
        "none",
        "ready",
        "observed_device_states.secret_state",
    );
    assert_observed_enum_insert_rejected(
        database,
        "bad-gateway-state",
        "applied",
        "installed",
        "unknown",
        "active",
        "none",
        "ready",
        "observed_device_states.gateway_state",
    );
    assert_observed_enum_insert_rejected(
        database,
        "bad-session-state",
        "applied",
        "installed",
        "ready",
        "unknown",
        "none",
        "ready",
        "observed_device_states.session_state",
    );
    assert_observed_enum_insert_rejected(
        database,
        "bad-session-lock-state",
        "applied",
        "installed",
        "ready",
        "active",
        "unknown",
        "ready",
        "observed_device_states.session_lock_state",
    );
    assert_observed_enum_insert_rejected(
        database,
        "bad-home-state",
        "applied",
        "installed",
        "ready",
        "active",
        "none",
        "unknown",
        "observed_device_states.home_state",
    );
    assert_rejected(
        database,
        "UPDATE observed_device_states SET session_agent_state = 'unknown' \
         WHERE device_pk = 'device-1'",
        "observed_device_states.session_agent_state",
    );
    assert_rejected(
        database,
        "UPDATE observed_device_states SET graphical_session_type = 'unknown' \
         WHERE device_pk = 'device-1'",
        "observed_device_states.graphical_session_type",
    );
    assert_rejected(
        database,
        "UPDATE observed_device_states SET display_backend = 'unknown' \
         WHERE device_pk = 'device-1'",
        "observed_device_states.display_backend",
    );
    assert_rejected(
        database,
        "UPDATE observed_device_states SET ui_presentation_state = 'unknown' \
         WHERE device_pk = 'device-1'",
        "observed_device_states.ui_presentation_state",
    );
    assert_rejected(
        database,
        "UPDATE observed_device_states SET session_screen_kind = 'unknown' \
         WHERE device_pk = 'device-1'",
        "observed_device_states.session_screen_kind",
    );
}

fn assert_session_credential_hash_domain(database: &TestDatabase) {
    assert_rejected(
        database,
        "INSERT INTO operator_sessions VALUES (zeroblob(32), 'operator-1', 'invalid')",
        "operator_sessions.expires_at malformed timestamp",
    );
    assert_rejected(
        database,
        "INSERT INTO operator_sessions VALUES (zeroblob(31), 'operator-1', \
         '2027-01-01T00:00:00.000Z')",
        "operator_sessions.session_credential_hash short length",
    );
    assert_rejected(
        database,
        "INSERT INTO operator_sessions VALUES (zeroblob(33), 'operator-1', \
         '2027-01-01T00:00:00.000Z')",
        "operator_sessions.session_credential_hash long length",
    );
    execute_fixture_statement(
        database,
        "INSERT INTO operator_sessions VALUES (zeroblob(32), 'operator-1', \
         '2027-01-01T00:00:00.000Z')",
        "a 32-byte session credential hash must be accepted",
    );
}

fn assert_binary_and_json_domains(database: &TestDatabase) {
    assert_rejected(
        database,
        "INSERT INTO device_tokens VALUES ('device-1', 'request-1', zeroblob(31))",
        "device_tokens.token_hash",
    );
    assert_rejected(
        database,
        "INSERT INTO enrollment_requests VALUES \
         ('bad-spki', 'hardware-4', 'strong', x'01', zeroblob(31), 'client', 1, \
          '192.0.2.1', 'pending', NULL, NULL, NULL, '2026-01-01T00:00:00.000Z')",
        "enrollment_requests.gateway_spki_sha256",
    );
    assert_rejected(
        database,
        "INSERT INTO pending_import_candidate VALUES \
         (1, 'candidate-1', '2027-01-01T00:00:00.000Z', 0, 0, zeroblob(31), \
          'vault-2', '{}')",
        "pending_import_candidate.preview_token_hash",
    );
    assert_rejected(
        database,
        "UPDATE observed_device_states SET applied_hash = zeroblob(31) \
         WHERE device_pk = 'device-1'",
        "observed_device_states.applied_hash",
    );
    assert_rejected(
        database,
        "UPDATE observed_device_states SET gateway_certificate_fingerprint = zeroblob(31) \
         WHERE device_pk = 'device-1'",
        "observed_device_states.gateway_certificate_fingerprint",
    );
    assert_rejected(
        database,
        "INSERT INTO gateway_certificates VALUES \
         ('bad-spki-certificate', 'device-1', 'request-1', 'serial-2', zeroblob(31), \
          '2027-01-01T00:00:00.000Z', 'active')",
        "gateway_certificates.spki_sha256",
    );
    assert_rejected(
        database,
        "INSERT INTO commands VALUES \
         ('bad-fingerprint', 'device-1', 'sync_state', 'queued', 1, zeroblob(31), \
          NULL, 1, '{}', '2026-01-01T00:00:00.000Z', '2026-01-02T00:00:00.000Z', \
          NULL, NULL, 'command-audit')",
        "commands.request_fingerprint_sha256",
    );
    assert_rejected(
        database,
        "INSERT INTO audit_events VALUES \
         ('bad-detail', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'operator:test', 'test', \
          'test', NULL, 'succeeded', NULL, 'correlation', NULL, '[]')",
        "audit_events.redacted_detail_json",
    );
    assert_rejected(
        database,
        "INSERT INTO commands VALUES \
         ('bad-payload', 'device-1', 'sync_state', 'queued', 1, zeroblob(32), \
          NULL, 1, '[]', '2026-01-01T00:00:00.000Z', '2026-01-02T00:00:00.000Z', \
          NULL, NULL, 'command-audit')",
        "commands.frozen_payload_json",
    );
    assert_rejected(
        database,
        "INSERT INTO pending_import_candidate VALUES \
         (1, 'candidate-1', '2027-01-01T00:00:00.000Z', 0, 0, zeroblob(32), \
          'vault-2', '[]')",
        "pending_import_candidate.redacted_preview_json",
    );
    assert_rejected(
        database,
        "INSERT INTO commands VALUES \
         ('bad-terminal-result', 'device-1', 'sync_state', 'failed', 1, zeroblob(32), \
          NULL, 1, '{}', '2026-01-01T00:00:00.000Z', '2026-01-02T00:00:00.000Z', \
          NULL, '[]', 'command-audit')",
        "commands.redacted_terminal_result_json",
    );
}

fn assert_numeric_singleton_and_consistency_domains(database: &TestDatabase) {
    assert_rejected(
        database,
        "INSERT INTO device_bindings VALUES ('seat-1', 'device-1', 0)",
        "device_bindings.binding_revision",
    );
    assert_rejected(
        database,
        "INSERT INTO accounts VALUES ('bad-revision', 'user-2', 'vault-3', 0)",
        "accounts.credential_revision",
    );
    assert_rejected(
        database,
        "UPDATE revision_counters SET configuration_revision = -1 WHERE singleton = 1",
        "revision_counters.configuration_revision",
    );
    assert_rejected(
        database,
        "UPDATE revision_counters SET binding_revision = -1 WHERE singleton = 1",
        "revision_counters.binding_revision",
    );
    assert_rejected(
        database,
        "UPDATE provisioning_window SET revision = -1 WHERE singleton = 1",
        "provisioning_window.revision",
    );
    assert_rejected(
        database,
        "INSERT INTO revision_counters VALUES (2, 0, 0)",
        "revision_counters.singleton",
    );
    assert_rejected(
        database,
        "INSERT INTO provisioning_window VALUES (2, 'closed', 0, NULL)",
        "provisioning_window.singleton",
    );
    assert_rejected(
        database,
        "UPDATE provisioning_window SET state = 'open', revision = 0, \
         last_audit_event_id = NULL WHERE singleton = 1",
        "provisioning_window consistency",
    );
}

fn assert_nonempty_and_version_domains(database: &TestDatabase) {
    assert_rejected(
        database,
        "INSERT INTO server_vault_records VALUES \
         ('empty-nonce', 'test', 'empty-nonce', zeroblob(0), x'01')",
        "server_vault_records.nonce",
    );
    assert_rejected(
        database,
        "INSERT INTO server_vault_records VALUES \
         ('empty-ciphertext', 'test', 'empty-ciphertext', x'01', zeroblob(0))",
        "server_vault_records.ciphertext",
    );
    assert_rejected(
        database,
        "INSERT INTO enrollment_requests VALUES \
         ('bad-enrollment-quality', 'hardware-5', 'unknown', x'01', zeroblob(32), \
          'client', 1, '192.0.2.1', 'pending', NULL, NULL, NULL, \
          '2026-01-01T00:00:00.000Z')",
        "enrollment_requests.hardware_identity_quality",
    );
    assert_rejected(
        database,
        "INSERT INTO enrollment_requests VALUES \
         ('empty-csr', 'hardware-6', 'strong', zeroblob(0), zeroblob(32), \
          'client', 1, '192.0.2.1', 'pending', NULL, NULL, NULL, \
          '2026-01-01T00:00:00.000Z')",
        "enrollment_requests.gateway_csr_der",
    );
    assert_rejected(
        database,
        "INSERT INTO enrollment_requests VALUES \
         ('negative-protocol', 'hardware-7', 'strong', x'01', zeroblob(32), \
          'client', -1, '192.0.2.1', 'pending', NULL, NULL, NULL, \
          '2026-01-01T00:00:00.000Z')",
        "enrollment_requests.protocol_version lower bound",
    );
    assert_rejected(
        database,
        "INSERT INTO enrollment_requests VALUES \
         ('oversized-protocol', 'hardware-8', 'strong', x'01', zeroblob(32), \
          'client', 4294967296, '192.0.2.1', 'pending', NULL, NULL, NULL, \
          '2026-01-01T00:00:00.000Z')",
        "enrollment_requests.protocol_version upper bound",
    );
    assert_rejected(
        database,
        "INSERT INTO commands VALUES \
         ('bad-request-version', 'device-1', 'sync_state', 'queued', 0, zeroblob(32), \
          NULL, 1, '{}', '2026-01-01T00:00:00.000Z', '2026-01-02T00:00:00.000Z', \
          NULL, NULL, 'command-audit')",
        "commands.request_fingerprint_version",
    );
    assert_rejected(
        database,
        "INSERT INTO commands VALUES \
         ('bad-payload-version', 'device-1', 'sync_state', 'queued', 1, zeroblob(32), \
          NULL, 0, '{}', '2026-01-01T00:00:00.000Z', '2026-01-02T00:00:00.000Z', \
          NULL, NULL, 'command-audit')",
        "commands.payload_version",
    );
}

fn assert_observed_numeric_and_boolean_domains(database: &TestDatabase) {
    let checks = [
        (
            "UPDATE observed_device_states SET observed_sequence = -1 WHERE device_pk = 'device-1'",
            "observed_device_states.observed_sequence",
        ),
        (
            "UPDATE observed_device_states SET received_generation = -1 WHERE device_pk = 'device-1'",
            "observed_device_states.received_generation",
        ),
        (
            "UPDATE observed_device_states SET applied_generation = -1 WHERE device_pk = 'device-1'",
            "observed_device_states.applied_generation",
        ),
        (
            "UPDATE observed_device_states SET installed_binding_revision = -1 \
             WHERE device_pk = 'device-1'",
            "observed_device_states.installed_binding_revision",
        ),
        (
            "UPDATE observed_device_states SET installed_credential_revision = -1 \
             WHERE device_pk = 'device-1'",
            "observed_device_states.installed_credential_revision",
        ),
        (
            "UPDATE observed_device_states SET gateway_configuration_revision = -1 \
             WHERE device_pk = 'device-1'",
            "observed_device_states.gateway_configuration_revision",
        ),
        (
            "UPDATE observed_device_states SET session_epoch = -1 WHERE device_pk = 'device-1'",
            "observed_device_states.session_epoch",
        ),
        (
            "UPDATE observed_device_states SET session_lock_epoch = -1 WHERE device_pk = 'device-1'",
            "observed_device_states.session_lock_epoch",
        ),
        (
            "UPDATE observed_device_states SET notifications_available = 2 \
             WHERE device_pk = 'device-1'",
            "observed_device_states.notifications_available",
        ),
        (
            "UPDATE observed_device_states SET desktop_lock_supported = 2 \
             WHERE device_pk = 'device-1'",
            "observed_device_states.desktop_lock_supported",
        ),
        (
            "UPDATE observed_device_states SET desktop_unlock_supported = 2 \
             WHERE device_pk = 'device-1'",
            "observed_device_states.desktop_unlock_supported",
        ),
    ];
    for (statement, contract) in checks {
        assert_rejected(database, statement, contract);
    }
}

fn assert_lifecycle_import_and_singleton_domains(database: &TestDatabase) {
    assert_rejected(
        database,
        "INSERT INTO site_identity VALUES (2, 'fleet-2')",
        "site_identity.singleton",
    );
    assert_rejected(
        database,
        "INSERT INTO pending_import_candidate VALUES \
         (2, 'candidate-2', '2027-01-01T00:00:00.000Z', 0, 0, zeroblob(32), \
          'vault-2', '{}')",
        "pending_import_candidate.singleton",
    );
    assert_rejected(
        database,
        "INSERT INTO pending_import_candidate VALUES \
         (1, 'negative-config-baseline', '2027-01-01T00:00:00.000Z', -1, 0, \
          zeroblob(32), 'vault-2', '{}')",
        "pending_import_candidate.baseline_configuration_revision",
    );
    assert_rejected(
        database,
        "INSERT INTO pending_import_candidate VALUES \
         (1, 'negative-binding-baseline', '2027-01-01T00:00:00.000Z', 0, -1, \
          zeroblob(32), 'vault-2', '{}')",
        "pending_import_candidate.baseline_binding_revision",
    );
    assert_rejected(
        database,
        "INSERT INTO enrollment_requests VALUES \
         ('issued-without-binding', 'hardware-9', 'strong', x'01', zeroblob(32), \
          'client', 1, '192.0.2.1', 'issued', NULL, NULL, NULL, \
          '2026-01-01T00:00:00.000Z')",
        "issued enrollment binding consistency",
    );
    assert_rejected(
        database,
        "INSERT INTO enrollment_requests VALUES \
         ('pending-with-audit', 'hardware-10', 'strong', x'01', zeroblob(32), \
          'client', 1, '192.0.2.1', 'pending', NULL, NULL, 'command-audit', \
          '2026-01-01T00:00:00.000Z')",
        "enrollment issuance audit consistency",
    );
}

async fn assert_connection_error_redaction() {
    let canary = format!("connection-redaction-canary-{}", Uuid::now_v7());
    let path = std::env::temp_dir().join(&canary).join("database.sqlite3");
    let error = require_database_error(
        Database::connect_and_migrate(&DatabaseConfig::new(path, true)).await,
        "connection canary unexpectedly connected",
    );
    assert_eq!(error, DatabaseError::ConnectionFailed);
    assert_database_error_is_redacted(error, &canary);
}

async fn assert_migration_error_redaction() {
    let canary = format!("migration-redaction-canary-{}", Uuid::now_v7());
    let fixture = TestDatabase::with_label(&canary);
    let mut connection = fixture.observer();
    require_ok(
        connection.batch_execute("CREATE TABLE site_identity (singleton INTEGER)"),
        "migration conflict fixture must insert",
    );
    drop(connection);

    let error = require_database_error(
        Database::connect_and_migrate(&fixture.config()).await,
        "migration canary unexpectedly migrated",
    );
    assert_eq!(error, DatabaseError::MigrationFailed);
    assert_database_error_is_redacted(error, &canary);
}

async fn assert_recovery_error_redaction() {
    let canary = format!("recovery-redaction-canary-{}", Uuid::now_v7());
    let fixture = TestDatabase::with_label(&canary);
    let database = fixture.connect().await;
    execute_fixture_statement(
        &fixture,
        "DELETE FROM provisioning_window",
        "recovery failure fixture must delete the singleton",
    );
    drop(database);

    let database = fixture.connect().await;
    let Err(error) = recover_on_startup(&database).await else {
        panic!("recovery canary unexpectedly recovered");
    };
    assert_eq!(error, ProvisioningError::PersistenceFailed);
    assert_provisioning_error_is_redacted(error, &canary);

    let mut observer = fixture.observer();
    for forbidden in [canary.as_str(), "SELECT", "INSERT", "provisioning_window"] {
        let audit_matches = require_ok(
            diesel::sql_query(
                "SELECT COUNT(*) AS value FROM audit_events WHERE instr( \
                 audit_event_id || occurred_at || actor || action_kind || resource_type || \
                 coalesce(resource_id, '') || result || coalesce(reason_code, '') || \
                 correlation_id || coalesce(group_correlation_id, '') || redacted_detail_json, \
                 ?) > 0",
            )
            .bind::<Text, _>(forbidden)
            .get_result::<IntegerRow>(&mut observer),
            "audit redaction evidence must be queryable",
        )
        .value;
        assert_eq!(audit_matches, 0);
    }
}

#[tokio::test]
async fn database_failures_redact_paths_sources_sql_and_audit_rows() {
    assert_connection_error_redaction().await;
    assert_migration_error_redaction().await;
    assert_recovery_error_redaction().await;
}

#[tokio::test]
async fn schema_constraints_reject_out_of_domain_values() {
    let fixture = TestDatabase::new();
    let _database = fixture.connect().await;
    seed_constraint_prerequisites(&fixture);

    assert_closed_enums(&fixture);
    assert_observed_closed_enums(&fixture);
    assert_session_credential_hash_domain(&fixture);
    assert_binary_and_json_domains(&fixture);
    assert_numeric_singleton_and_consistency_domains(&fixture);
    assert_nonempty_and_version_domains(&fixture);
    assert_observed_numeric_and_boolean_domains(&fixture);
    assert_lifecycle_import_and_singleton_domains(&fixture);
}

#[tokio::test]
async fn unique_current_fact_constraints_reject_duplicates() {
    let fixture = TestDatabase::new();
    let _database = fixture.connect().await;
    seed_constraint_prerequisites(&fixture);

    assert_rejected(
        &fixture,
        "INSERT INTO server_vault_records VALUES \
         ('vault-4', 'account', 'subject-1', x'01', x'02')",
        "server_vault_records current subject",
    );
    assert_rejected(
        &fixture,
        "INSERT INTO seats VALUES ('seat-2', 'S1')",
        "seats.seat_code",
    );
    assert_rejected(
        &fixture,
        "INSERT INTO devices VALUES ('device-2', 'hardware-1', 'strong', 'enrolled')",
        "devices.machine_hardware_id",
    );
    assert_rejected(
        &fixture,
        "INSERT INTO operator_accounts VALUES \
         ('operator-2', 'admin-1', 'viewer', 'work-factor-hash')",
        "operator_accounts.login_name",
    );
    assert_rejected(
        &fixture,
        "INSERT INTO accounts VALUES ('account-2', 'user-1', 'vault-3', 1)",
        "accounts.domjudge_username",
    );
    assert_rejected(
        &fixture,
        "INSERT INTO enrollment_requests VALUES \
         ('request-2', 'hardware-1', 'strong', x'01', zeroblob(32), 'client', 1, \
          '192.0.2.1', 'pending', NULL, NULL, NULL, '2026-01-01T00:00:00.000Z')",
        "pending live enrollment hardware and SPKI",
    );
    assert_rejected(
        &fixture,
        "INSERT INTO enrollment_requests VALUES \
         ('request-approved', 'hardware-1', 'strong', x'01', zeroblob(32), 'client', 1, \
          '192.0.2.1', 'approved', NULL, NULL, NULL, '2026-01-01T00:00:00.000Z')",
        "approved live enrollment hardware and SPKI",
    );

    execute_fixture_statement(
        &fixture,
        "INSERT INTO devices VALUES ('device-2', 'hardware-2', 'strong', 'enrolled')",
        "second device must insert",
    );
    execute_fixture_statement(
        &fixture,
        "INSERT INTO enrollment_requests VALUES \
         ('request-3', 'hardware-2', 'strong', x'01', zeroblob(32), 'client', 1, \
          '192.0.2.1', 'pending', NULL, NULL, NULL, '2026-01-01T00:00:00.000Z')",
        "second enrollment request must insert",
    );
    execute_fixture_statement(
        &fixture,
        "INSERT INTO gateway_certificates VALUES \
         ('certificate-1', 'device-1', 'request-1', 'serial-1', zeroblob(32), \
          '2027-01-01T00:00:00.000Z', 'active')",
        "first active certificate must insert",
    );
    assert_rejected(
        &fixture,
        "INSERT INTO gateway_certificates VALUES \
         ('certificate-2', 'device-1', 'request-3', 'serial-2', zeroblob(32), \
          '2027-01-01T00:00:00.000Z', 'active')",
        "one active gateway certificate per device",
    );
}

#[tokio::test]
async fn enrollment_resolution_requires_one_composite_device_identity() {
    let fixture = TestDatabase::new();
    let _database = fixture.connect().await;
    seed_constraint_prerequisites(&fixture);
    execute_fixture_statement(
        &fixture,
        "INSERT INTO devices VALUES ('device-2', 'hardware-2', 'strong', 'enrolled')",
        "second device must insert",
    );
    execute_fixture_statement(
        &fixture,
        "INSERT INTO audit_events VALUES \
         ('issuance-audit', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'operator:test', \
          'issue_enrollment', 'enrollment', 'cross-device-request', 'succeeded', NULL, \
          'issuance-correlation', NULL, '{}')",
        "issuance audit must insert",
    );
    assert_rejected(
        &fixture,
        "INSERT INTO enrollment_requests VALUES \
         ('cross-device-request', 'hardware-2', 'strong', x'01', zeroblob(32), 'client', 1, \
          '192.0.2.1', 'issued', 'replace_device_credentials', 'device-1', \
          'issuance-audit', '2026-01-01T00:00:00.000Z')",
        "enrollment composite device identity",
    );
}

fn assert_table_column_and_strict_contract(database: &TestDatabase) {
    let expected = expected_columns();
    let expected_tables: Vec<String> = expected.keys().map(ToString::to_string).collect();
    assert_eq!(application_tables(database), expected_tables);

    for (table, expected_table_columns) in expected {
        let expected_table_columns: Vec<String> = expected_table_columns
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(columns(database, table), expected_table_columns);
    }

    let mut connection = database.observer();
    let strict_rows = require_ok(
        diesel::sql_query(
            "SELECT strict AS value FROM pragma_table_list \
             WHERE schema = 'main' AND name NOT LIKE 'sqlite_%' \
             AND name <> '__diesel_schema_migrations' ORDER BY name",
        )
        .load::<IntegerRow>(&mut connection),
        "strict flags must be queryable",
    );
    assert_eq!(strict_rows.len(), 18);
    assert!(strict_rows.iter().all(|row| row.value == 1));
}

fn assert_invariant_index_contract(database: &TestDatabase) {
    let mut connection = database.observer();
    let explicit_indexes: Vec<String> = require_ok(
        diesel::sql_query(
            "SELECT name AS value FROM sqlite_schema \
             WHERE type = 'index' AND sql IS NOT NULL ORDER BY name",
        )
        .load::<TextRow>(&mut connection),
        "index names must be queryable",
    )
    .into_iter()
    .map(|row| row.value)
    .collect();
    assert_eq!(
        explicit_indexes,
        vec![
            "device_pk_machine_hardware_identity",
            "one_active_gateway_certificate",
            "one_live_enrollment_per_machine_and_gateway_spki",
        ]
    );
    assert_eq!(
        index_properties(database, "devices", "device_pk_machine_hardware_identity"),
        (true, false)
    );
    assert_eq!(
        index_properties(
            database,
            "gateway_certificates",
            "one_active_gateway_certificate",
        ),
        (true, true)
    );
    assert_eq!(
        index_properties(
            database,
            "enrollment_requests",
            "one_live_enrollment_per_machine_and_gateway_spki",
        ),
        (true, true)
    );
}

fn assert_foreign_key_contract(database: &TestDatabase) {
    let expected_foreign_keys = expected_foreign_keys();
    for table in application_tables(database) {
        let actual = foreign_keys(database, &table);
        let expected: Vec<(String, String, String)> = expected_foreign_keys
            .get(table.as_str())
            .into_iter()
            .flatten()
            .map(|(source, target_table, target_column)| {
                (
                    (*source).to_owned(),
                    (*target_table).to_owned(),
                    (*target_column).to_owned(),
                )
            })
            .collect();
        assert_eq!(actual, expected, "unexpected foreign keys on {table}");
    }
    assert_eq!(
        enrollment_device_foreign_key_groups(database),
        vec![
            vec![(0, "resolved_device_pk".to_owned(), "device_pk".to_owned())],
            vec![
                (0, "resolved_device_pk".to_owned(), "device_pk".to_owned()),
                (
                    1,
                    "machine_hardware_id".to_owned(),
                    "machine_hardware_id".to_owned(),
                ),
            ],
        ]
    );
}

fn assert_seed_trigger_wal_and_fk_behavior(database: &TestDatabase) {
    let mut connection = database.observer();
    let trigger_count = require_ok(
        diesel::sql_query("SELECT COUNT(*) AS value FROM sqlite_schema WHERE type = 'trigger'")
            .get_result::<IntegerRow>(&mut connection),
        "trigger count must be queryable",
    )
    .value;
    assert_eq!(trigger_count, 0);

    let revisions = require_ok(
        diesel::sql_query(
            "SELECT singleton, configuration_revision, binding_revision FROM revision_counters",
        )
        .get_result::<RevisionCountersRow>(&mut connection),
        "revision singleton must exist",
    );
    assert_eq!(
        (
            revisions.singleton,
            revisions.configuration_revision,
            revisions.binding_revision,
        ),
        (1, 0, 0)
    );
    let window = require_ok(
        diesel::sql_query(
            "SELECT singleton, state, revision, last_audit_event_id FROM provisioning_window",
        )
        .get_result::<ProvisioningWindowRow>(&mut connection),
        "provisioning-window singleton must exist",
    );
    assert_eq!(
        (
            window.singleton,
            window.state,
            window.revision,
            window.last_audit_event_id,
        ),
        (1, "closed".to_owned(), 0, None)
    );

    let journal_mode = require_ok(
        diesel::sql_query("SELECT journal_mode AS value FROM pragma_journal_mode")
            .get_result::<TextRow>(&mut connection),
        "journal mode must be queryable",
    )
    .value;
    assert_eq!(journal_mode, "wal");
    assert_foreign_key_enforcement(database);
}

#[tokio::test]
async fn migration_is_idempotent_and_schema_contract_is_exact() {
    let fixture = TestDatabase::new();
    drop(fixture.connect().await);

    let _database = fixture.connect().await;
    assert_table_column_and_strict_contract(&fixture);
    assert_invariant_index_contract(&fixture);
    assert_foreign_key_contract(&fixture);
    assert_seed_trigger_wal_and_fk_behavior(&fixture);
}

fn assert_recovery_audit(database: &TestDatabase) -> String {
    let mut connection = database.observer();
    let recovery_audit_count = require_ok(
        diesel::sql_query(
            "SELECT COUNT(*) AS value FROM audit_events WHERE actor = 'system:recovery'",
        )
        .get_result::<IntegerRow>(&mut connection),
        "recovery audit count must be queryable",
    )
    .value;
    assert_eq!(recovery_audit_count, 2);
    let recovery_audits = require_ok(
        diesel::sql_query(
            "SELECT audit_event_id, actor, action_kind, result, reason_code, correlation_id, \
             redacted_detail_json FROM audit_events WHERE actor = 'system:recovery' \
             ORDER BY rowid",
        )
        .load::<RecoveryAuditRow>(&mut connection),
        "recovery audits must be queryable",
    );
    assert_eq!(recovery_audits.len(), 2);
    let recovery_close = &recovery_audits[0];
    let enrollment_expiry = &recovery_audits[1];
    assert_eq!(recovery_close.actor, "system:recovery");
    assert_eq!(recovery_close.action_kind, "close_provisioning_window");
    assert_eq!(recovery_close.result, "succeeded");
    assert_eq!(
        recovery_close.reason_code.as_deref(),
        Some("startup_recovery")
    );
    let recovery_detail: serde_json::Value = require_ok(
        serde_json::from_str(&recovery_close.redacted_detail_json),
        "recovery audit detail must be valid JSON",
    );
    assert_eq!(
        recovery_detail,
        serde_json::json!({"previous_revision": 1, "new_revision": 2})
    );
    assert_eq!(enrollment_expiry.actor, "system:recovery");
    assert_eq!(enrollment_expiry.action_kind, "expire_enrollment_requests");
    assert_eq!(enrollment_expiry.result, "succeeded");
    assert_eq!(
        enrollment_expiry.reason_code.as_deref(),
        Some("window_closed")
    );
    assert_eq!(
        require_ok(
            serde_json::from_str::<serde_json::Value>(&enrollment_expiry.redacted_detail_json),
            "Enrollment expiry audit detail must be valid JSON",
        ),
        serde_json::json!({"expired_count": 0})
    );
    assert_eq!(
        enrollment_expiry.correlation_id,
        recovery_close.correlation_id
    );
    let canonical_timestamp_count = require_ok(
        diesel::sql_query(
            "SELECT COUNT(*) AS value FROM audit_events WHERE actor = 'system:recovery' \
             AND occurred_at = strftime('%Y-%m-%dT%H:%M:%fZ', occurred_at)",
        )
        .get_result::<IntegerRow>(&mut connection),
        "recovery audit timestamps must be queryable",
    )
    .value;
    assert_eq!(canonical_timestamp_count, 2);
    recovery_close.audit_event_id.clone()
}

#[tokio::test]
async fn startup_recovery_closes_an_open_window_exactly_once() {
    let fixture = TestDatabase::new();
    let database = fixture.connect().await;
    insert_open_window_fixture(&fixture, Uuid::now_v7());
    drop(database);

    let database = fixture.connect().await;
    let first_outcome = require_ok(
        recover_on_startup(&database).await,
        "first recovery must succeed",
    );
    assert!(matches!(
        first_outcome,
        RecoveryOutcome::Closed {
            previous_revision: 1,
            new_revision: 2,
            ..
        }
    ));
    let mut connection = fixture.observer();
    let recovered_window_row = require_ok(
        diesel::sql_query(
            "SELECT singleton, state, revision, last_audit_event_id \
             FROM provisioning_window WHERE singleton = 1",
        )
        .get_result::<ProvisioningWindowRow>(&mut connection),
        "recovered window must be queryable",
    );
    let recovered_window = (
        recovered_window_row.state,
        recovered_window_row.revision,
        recovered_window_row.last_audit_event_id,
    );
    assert_eq!(recovered_window.0, "closed");
    assert_eq!(recovered_window.1, 2);
    let recovery_audit_id = assert_recovery_audit(&fixture);
    assert_eq!(
        recovered_window.2.as_deref(),
        Some(recovery_audit_id.as_str())
    );

    let audit_count_before = require_ok(
        diesel::sql_query("SELECT COUNT(*) AS value FROM audit_events")
            .get_result::<IntegerRow>(&mut connection),
        "audit count must be queryable",
    )
    .value;
    let mut observer = fixture.observer();
    let data_version_before = require_ok(
        diesel::sql_query("SELECT data_version AS value FROM pragma_data_version")
            .get_result::<IntegerRow>(&mut observer),
        "database version must be queryable",
    )
    .value;
    let second_outcome = require_ok(
        recover_on_startup(&database).await,
        "second recovery must succeed",
    );
    assert_eq!(
        second_outcome,
        RecoveryOutcome::AlreadyClosed { revision: 2 }
    );
    let window_after_row = require_ok(
        diesel::sql_query(
            "SELECT singleton, state, revision, last_audit_event_id \
             FROM provisioning_window WHERE singleton = 1",
        )
        .get_result::<ProvisioningWindowRow>(&mut connection),
        "provisioning window must be queryable",
    );
    let window_after = (
        window_after_row.state,
        window_after_row.revision,
        window_after_row.last_audit_event_id,
    );
    let audit_count_after = require_ok(
        diesel::sql_query("SELECT COUNT(*) AS value FROM audit_events")
            .get_result::<IntegerRow>(&mut connection),
        "audit count must be queryable",
    )
    .value;
    let data_version_after = require_ok(
        diesel::sql_query("SELECT data_version AS value FROM pragma_data_version")
            .get_result::<IntegerRow>(&mut observer),
        "database version must be queryable",
    )
    .value;
    assert_eq!(window_after, recovered_window);
    assert_eq!(audit_count_after, audit_count_before);
    assert_eq!(data_version_after, data_version_before);
}

#[tokio::test]
async fn startup_recovery_revision_overflow_is_distinct_and_zero_write() {
    let fixture = TestDatabase::new();
    let database = fixture.connect().await;
    let opening_audit_id = Uuid::now_v7();
    insert_open_window_fixture(&fixture, opening_audit_id);

    let mut connection = fixture.observer();
    require_ok(
        diesel::sql_query(
            "UPDATE provisioning_window SET revision = ? \
             WHERE singleton = 1 AND state = 'open'",
        )
        .bind::<BigInt, _>(i64::MAX)
        .execute(&mut connection),
        "overflow fixture revision must update with an explicit BigInt binding",
    );
    let audit_count_before = require_ok(
        diesel::sql_query("SELECT COUNT(*) AS value FROM audit_events")
            .get_result::<IntegerRow>(&mut connection),
        "audit count must be queryable",
    )
    .value;
    let mut observer = fixture.observer();
    let data_version_before = require_ok(
        diesel::sql_query("SELECT data_version AS value FROM pragma_data_version")
            .get_result::<IntegerRow>(&mut observer),
        "database version must be queryable",
    )
    .value;

    let Err(error) = recover_on_startup(&database).await else {
        panic!("overflow recovery unexpectedly succeeded");
    };
    assert_eq!(error, ProvisioningError::RevisionOverflow);
    assert_eq!(
        error.to_string(),
        "the provisioning window revision cannot be incremented"
    );
    assert_provisioning_error_is_redacted(error, "overflow-recovery-canary");

    let window_after = require_ok(
        diesel::sql_query(
            "SELECT singleton, state, revision, last_audit_event_id \
             FROM provisioning_window WHERE singleton = 1",
        )
        .get_result::<ProvisioningWindowRow>(&mut connection),
        "overflowed provisioning window must be queryable",
    );
    let audit_count_after = require_ok(
        diesel::sql_query("SELECT COUNT(*) AS value FROM audit_events")
            .get_result::<IntegerRow>(&mut connection),
        "audit count must be queryable",
    )
    .value;
    let data_version_after = require_ok(
        diesel::sql_query("SELECT data_version AS value FROM pragma_data_version")
            .get_result::<IntegerRow>(&mut observer),
        "database version must be queryable",
    )
    .value;

    assert_eq!(window_after.state, "open");
    assert_eq!(window_after.revision, i64::MAX);
    assert_eq!(
        window_after.last_audit_event_id.as_deref(),
        Some(opening_audit_id.to_string().as_str())
    );
    assert_eq!(audit_count_after, audit_count_before);
    assert_eq!(data_version_after, data_version_before);
}
