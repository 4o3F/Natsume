use std::{error::Error, path::Path};

use diesel::{
    Connection, RunQueryDsl,
    connection::SimpleConnection,
    dsl::sql,
    result::{DatabaseErrorKind, Error as DieselError},
    sql_types::BigInt,
    sqlite::SqliteConnection,
};
use diesel_migrations::{FileBasedMigrations, MigrationHarness};

type TestError = Box<dyn Error + Send + Sync>;

fn migrations() -> Result<FileBasedMigrations, TestError> {
    Ok(FileBasedMigrations::from_path(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
    )?)
}

fn database() -> Result<SqliteConnection, TestError> {
    let mut connection = SqliteConnection::establish(":memory:")?;
    connection.batch_execute("PRAGMA foreign_keys = ON;")?;
    assert_eq!(connection.run_pending_migrations(migrations()?)?.len(), 6);
    Ok(connection)
}

fn rejects(connection: &mut SqliteConnection, statement: &str, kind: DatabaseErrorKind) {
    match connection.batch_execute(statement) {
        Err(DieselError::DatabaseError(actual, _)) => assert_eq!(actual, kind, "{statement}"),
        result => panic!("expected {kind:?} for {statement}, got {result:?}"),
    }
}

#[test]
fn migrations_round_trip() -> Result<(), TestError> {
    let mut connection = database()?;
    let table_count = "SELECT count(*) FROM sqlite_schema WHERE type = 'table' \
        AND name NOT LIKE 'sqlite_%' AND name != '__diesel_schema_migrations'";
    assert_eq!(
        sql::<BigInt>(table_count).get_result::<i64>(&mut connection)?,
        22
    );
    assert_eq!(connection.revert_all_migrations(migrations()?)?.len(), 6);
    assert_eq!(
        sql::<BigInt>(table_count).get_result::<i64>(&mut connection)?,
        0
    );
    assert_eq!(connection.run_pending_migrations(migrations()?)?.len(), 6);
    assert_eq!(
        sql::<BigInt>(table_count).get_result::<i64>(&mut connection)?,
        22
    );
    Ok(())
}

#[test]
fn network_migration_preserves_existing_devices_with_unknown_addresses() -> Result<(), TestError> {
    let mut connection = SqliteConnection::establish(":memory:")?;
    for _ in 0..4 {
        connection.run_next_migration(migrations()?)?;
    }
    connection.batch_execute(
        "INSERT INTO devices VALUES ('d1', 'machine-1', 'strong', 'enabled', 12);",
    )?;
    connection.run_next_migration(migrations()?)?;
    assert_eq!(sql::<BigInt>("SELECT count(*) FROM devices WHERE device_id = 'd1' AND machine_hardware_id = 'machine-1' AND state = 'enabled' AND created_at_unix_ms = 12 AND client_ip IS NULL AND server_observed_ip IS NULL AND ip_observed_at_unix_ms IS NULL").get_result::<i64>(&mut connection)?, 1);
    connection.batch_execute("UPDATE devices SET client_ip = '192.0.2.1', server_observed_ip = '192.0.2.2', ip_observed_at_unix_ms = 1700000000000 WHERE device_id = 'd1';")?;
    assert_eq!(
        sql::<BigInt>("SELECT ip_observed_at_unix_ms FROM devices WHERE device_id = 'd1'")
            .get_result::<i64>(&mut connection)?,
        1_700_000_000_000
    );
    connection.revert_last_migration(migrations()?)?;
    assert_eq!(
        sql::<BigInt>("SELECT created_at_unix_ms FROM devices WHERE device_id = 'd1'")
            .get_result::<i64>(&mut connection)?,
        12
    );
    Ok(())
}

#[test]
fn target_submission_ids_remain_reserved_after_operator_and_device_removal() -> Result<(), TestError>
{
    let mut connection = database()?;
    connection.batch_execute("INSERT INTO operator_accounts VALUES ('o1','operator','admin','phc',1); INSERT INTO devices (device_id, machine_hardware_id, evidence_quality, state, created_at_unix_ms) VALUES ('d1','m1','strong','enabled',1); INSERT INTO target_submission_receipts VALUES ('r1','o1','request','result'); DELETE FROM devices; DELETE FROM operator_accounts;")?;
    assert_eq!(
        sql::<BigInt>("SELECT count(*) FROM target_submission_receipts")
            .get_result::<i64>(&mut connection)?,
        1
    );
    rejects(
        &mut connection,
        "INSERT INTO target_submission_receipts VALUES ('r1','o2','other','other');",
        DatabaseErrorKind::UniqueViolation,
    );
    Ok(())
}

#[test]
fn operator_links_migration_preserves_existing_deployment_data() -> Result<(), TestError> {
    let mut connection = SqliteConnection::establish(":memory:")?;
    connection.batch_execute("PRAGMA foreign_keys = ON;")?;
    for _ in 0..5 {
        connection.run_next_migration(migrations()?)?;
    }
    connection.batch_execute(
        "
        INSERT INTO site_identity VALUES (1, 'fleet');
        INSERT INTO operator_accounts VALUES ('o1', ' legacy admin ', 'admin', 'legacy-phc', 5), ('o2', 'viewer', 'viewer', 'viewer-phc', 2);
        INSERT INTO operator_sessions VALUES (x'10', 'o1', 1700000000000);
        INSERT INTO seats VALUES ('s1', 'A-01');
        INSERT INTO accounts VALUES ('a1', 'team-1', 7);
        INSERT INTO server_vault_records VALUES ('a1', x'0102', x'0304');
        INSERT INTO account_mappings VALUES ('s1', 'a1');
        INSERT INTO organizations VALUES (1, 'University', '', 'University', 'CHN');
        INSERT INTO teams VALUES ('a1', 1, '队伍', 'Team', 'participant');
        INSERT INTO devices VALUES ('d1', 'machine-1', 'strong', 'enabled', 12, '192.0.2.1', '192.0.2.2', 1700000000000);
        INSERT INTO device_bindings VALUES ('b1', 'd1', 's1');
        INSERT INTO device_control_keys VALUES (x'05', 'd1', 'current', 13, NULL);
        INSERT INTO gateway_credentials VALUES ('d1', 'g1', x'06', x'07');
        INSERT INTO binding_negotiations VALUES ('d1', 'n1', 8, 'A-01', NULL);
        INSERT INTO device_session_targets VALUES ('d1', 'contest', 9);
        INSERT INTO device_home_targets VALUES ('d1', 10);
        INSERT INTO device_power_targets VALUES ('d1', 11, 1700000000000);
        INSERT INTO runtime_config VALUES (1, 'https://judge.example');
        INSERT INTO pending_import_candidate VALUES (1, 'p1', 99, x'08', 1, x'09', x'0a', '{}');
        INSERT INTO target_submission_receipts VALUES ('r1', 'o1', 'request', 'result');
    ",
    )?;
    let checks = [
        "SELECT count(*) FROM site_identity WHERE singleton = 1 AND fleet_namespace_uuid = 'fleet'",
        "SELECT count(*) FROM operator_accounts WHERE operator_id = 'o1' AND username = ' legacy admin ' AND role = 'admin' AND password_hash = 'legacy-phc' AND credential_revision = 5",
        "SELECT count(*) FROM operator_accounts WHERE operator_id = 'o2' AND username = 'viewer' AND role = 'viewer' AND password_hash = 'viewer-phc' AND credential_revision = 2",
        "SELECT count(*) FROM operator_sessions WHERE session_credential_hash = x'10' AND operator_id = 'o1' AND expires_at_unix_ms = 1700000000000",
        "SELECT count(*) FROM accounts a JOIN server_vault_records v USING (account_id) JOIN account_mappings m USING (account_id) JOIN seats s USING (seat_id) WHERE a.account_id = 'a1' AND domjudge_username = 'team-1' AND credential_revision = 7 AND nonce = x'0102' AND ciphertext = x'0304' AND s.seat_id = 's1' AND seat_code = 'A-01'",
        "SELECT count(*) FROM organizations WHERE organization_id = 1 AND name_key = 'University' AND name_zh = '' AND name_en = 'University' AND country = 'CHN'",
        "SELECT count(*) FROM teams WHERE account_id = 'a1' AND organization_id = 1 AND name_zh = '队伍' AND name_en = 'Team' AND category = 'participant'",
        "SELECT count(*) FROM device_bindings b JOIN devices d USING (device_id) WHERE binding_id = 'b1' AND device_id = 'd1' AND seat_id = 's1' AND machine_hardware_id = 'machine-1' AND evidence_quality = 'strong' AND state = 'enabled' AND created_at_unix_ms = 12 AND client_ip = '192.0.2.1' AND server_observed_ip = '192.0.2.2' AND ip_observed_at_unix_ms = 1700000000000",
        "SELECT count(*) FROM device_control_keys WHERE public_key = x'05' AND device_id = 'd1' AND status = 'current' AND activated_at_unix_ms = 13 AND retired_at_unix_ms IS NULL",
        "SELECT count(*) FROM gateway_credentials WHERE device_id = 'd1' AND credential_id = 'g1' AND gateway_csr_der = x'06' AND gateway_leaf_der = x'07'",
        "SELECT count(*) FROM binding_negotiations WHERE device_id = 'd1' AND negotiation_id = 'n1' AND submission_epoch = 8 AND seat_code = 'A-01' AND evaluation_error_code IS NULL",
        "SELECT count(*) FROM device_session_targets WHERE device_id = 'd1' AND foreground_target = 'contest' AND terminate_epoch = 9",
        "SELECT count(*) FROM device_home_targets WHERE device_id = 'd1' AND reset_epoch = 10",
        "SELECT count(*) FROM device_power_targets WHERE device_id = 'd1' AND shutdown_epoch = 11 AND expires_at_unix_ms = 1700000000000",
        "SELECT count(*) FROM runtime_config WHERE singleton = 1 AND domjudge_origin = 'https://judge.example'",
        "SELECT count(*) FROM pending_import_candidate WHERE singleton = 1 AND candidate_id = 'p1' AND expires_at_unix_ms = 99 AND preview_token_hash = x'08' AND fingerprint_version = 1 AND candidate_fingerprint_sha256 = x'09' AND baseline_fingerprint_sha256 = x'0a' AND redacted_preview_json = '{}'",
        "SELECT count(*) FROM target_submission_receipts WHERE operation_id = 'r1' AND operator_id = 'o1' AND request_json = 'request' AND results_json = 'result'",
    ];
    assert_eq!(connection.run_pending_migrations(migrations()?)?.len(), 1);
    assert!(connection.run_pending_migrations(migrations()?)?.is_empty());
    for statement in checks {
        assert_eq!(
            sql::<BigInt>(statement).get_result::<i64>(&mut connection)?,
            1
        );
    }
    for table in ["operator_invitations", "operator_password_resets"] {
        assert_eq!(
            sql::<BigInt>(&format!("SELECT count(*) FROM {table}"))
                .get_result::<i64>(&mut connection)?,
            0
        );
    }
    connection.revert_last_migration(migrations()?)?;
    for statement in checks {
        assert_eq!(
            sql::<BigInt>(statement).get_result::<i64>(&mut connection)?,
            1
        );
    }
    Ok(())
}

#[test]
fn operator_links_enforce_unique_identifiers_hashes_and_reset_targets() -> Result<(), TestError> {
    let mut connection = database()?;
    connection.batch_execute(
        "
        INSERT INTO operator_accounts VALUES ('issuer', 'admin', 'admin', 'phc', 1), ('u1', 'user1', 'viewer', 'phc', 1), ('u2', 'user2', 'viewer', 'phc', 1);
        INSERT INTO operator_invitations VALUES ('i1', 'viewer', 'issuer', x'01', 1, 2);
        INSERT INTO operator_password_resets VALUES ('u1', 'r1', 'issuer', x'02', 1, 2);
    ",
    )?;
    for statement in [
        "INSERT INTO operator_invitations VALUES ('i1', 'admin', 'issuer', x'03', 1, 2)",
        "INSERT INTO operator_invitations VALUES ('i2', 'viewer', 'issuer', x'01', 1, 2)",
        "INSERT INTO operator_password_resets VALUES ('u1', 'r2', 'issuer', x'03', 1, 2)",
        "INSERT INTO operator_password_resets VALUES ('u2', 'r1', 'issuer', x'03', 1, 2)",
        "INSERT INTO operator_password_resets VALUES ('u2', 'r2', 'issuer', x'02', 1, 2)",
    ] {
        rejects(
            &mut connection,
            statement,
            DatabaseErrorKind::UniqueViolation,
        );
    }
    for statement in [
        "INSERT INTO operator_invitations VALUES (NULL, 'viewer', 'issuer', x'03', 1, 2)",
        "INSERT INTO operator_invitations VALUES ('i2', 'viewer', 'issuer', NULL, 1, 2)",
        "INSERT INTO operator_password_resets VALUES (NULL, 'r2', 'issuer', x'03', 1, 2)",
        "INSERT INTO operator_password_resets VALUES ('u2', 'r2', 'issuer', NULL, 1, 2)",
    ] {
        rejects(
            &mut connection,
            statement,
            DatabaseErrorKind::NotNullViolation,
        );
    }
    connection.batch_execute(
        "
        INSERT INTO operator_invitations VALUES ('i2', 'admin', 'issuer', x'03', 1700000000000, 1700604800000);
        INSERT INTO operator_password_resets VALUES ('u2', 'r2', 'issuer', x'04', 1700000000000, 1700003600000);
    ",
    )?;
    assert_eq!(
        sql::<BigInt>(
            "SELECT expires_at_unix_ms FROM operator_invitations WHERE invitation_id = 'i2'"
        )
        .get_result::<i64>(&mut connection)?,
        1_700_604_800_000
    );
    assert_eq!(
        sql::<BigInt>(
            "SELECT expires_at_unix_ms FROM operator_password_resets WHERE operator_id = 'u2'"
        )
        .get_result::<i64>(&mut connection)?,
        1_700_003_600_000
    );
    Ok(())
}

#[test]
fn operator_link_foreign_keys_cascade_by_target_and_issuer() -> Result<(), TestError> {
    let mut connection = database()?;
    connection.batch_execute(
        "
        INSERT INTO operator_accounts VALUES ('issuer', 'admin', 'admin', 'phc', 1), ('u1', 'user1', 'viewer', 'phc', 1), ('u2', 'user2', 'viewer', 'phc', 1);
        INSERT INTO operator_invitations VALUES ('i1', 'viewer', 'issuer', x'01', 1, 2);
        INSERT INTO operator_password_resets VALUES ('u1', 'r1', 'issuer', x'02', 1, 2), ('u2', 'r2', 'issuer', x'03', 1, 2);
    ",
    )?;
    for statement in [
        "INSERT INTO operator_invitations VALUES ('i2', 'viewer', 'missing', x'04', 1, 2)",
        "INSERT INTO operator_password_resets VALUES ('missing', 'r3', 'issuer', x'04', 1, 2)",
        "UPDATE operator_password_resets SET issuer_operator_id = 'missing' WHERE operator_id = 'u1'",
    ] {
        rejects(
            &mut connection,
            statement,
            DatabaseErrorKind::ForeignKeyViolation,
        );
    }
    connection.batch_execute(
        "DELETE FROM operator_accounts WHERE operator_id = 'u1'; INSERT INTO operator_accounts VALUES ('u1-new', 'user1', 'viewer', 'phc', 1);",
    )?;
    assert_eq!(
        sql::<BigInt>("SELECT count(*) FROM operator_password_resets WHERE operator_id = 'u2'")
            .get_result::<i64>(&mut connection)?,
        1
    );
    assert_eq!(
        sql::<BigInt>(
            "SELECT count(*) FROM operator_password_resets WHERE operator_id IN ('u1', 'u1-new')"
        )
        .get_result::<i64>(&mut connection)?,
        0
    );
    assert_eq!(
        sql::<BigInt>("SELECT count(*) FROM operator_invitations")
            .get_result::<i64>(&mut connection)?,
        1
    );
    connection.batch_execute("DELETE FROM operator_accounts WHERE operator_id = 'issuer';")?;
    for table in ["operator_invitations", "operator_password_resets"] {
        assert_eq!(
            sql::<BigInt>(&format!("SELECT count(*) FROM {table}"))
                .get_result::<i64>(&mut connection)?,
            0
        );
    }
    assert_eq!(
        sql::<BigInt>("SELECT count(*) FROM operator_accounts")
            .get_result::<i64>(&mut connection)?,
        2
    );
    Ok(())
}

#[test]
fn strict_tables_enforce_storage_types_and_nullability() -> Result<(), TestError> {
    let mut connection = database()?;
    assert_eq!(
        sql::<BigInt>(
            "SELECT count(*) FROM pragma_table_list WHERE schema = 'main' \
        AND name NOT LIKE 'sqlite_%' AND name != '__diesel_schema_migrations' AND strict = 1"
        )
        .get_result::<i64>(&mut connection)?,
        22
    );
    connection.batch_execute("INSERT INTO accounts VALUES ('a1', 'team-1', 1);")?;
    rejects(
        &mut connection,
        "INSERT INTO accounts VALUES ('a2', NULL, 1);",
        DatabaseErrorKind::NotNullViolation,
    );
    rejects(
        &mut connection,
        "INSERT INTO accounts VALUES (NULL, 'team-2', 1);",
        DatabaseErrorKind::NotNullViolation,
    );
    let error =
        connection.batch_execute("UPDATE accounts SET credential_revision = 'not-an-integer';");
    match error {
        Err(DieselError::DatabaseError(_, detail)) => {
            assert!(detail.message().contains(
                "cannot store TEXT value in INTEGER column accounts.credential_revision"
            ));
        }
        result => panic!("expected a SQLite storage type violation, got {result:?}"),
    }
    Ok(())
}

#[test]
fn foreign_keys_reject_orphans_and_cascade_owned_rows() -> Result<(), TestError> {
    let mut connection = database()?;
    for statement in [
        "INSERT INTO operator_sessions VALUES (x'01', 'missing', 1);",
        "INSERT INTO server_vault_records VALUES ('missing', x'01', x'02');",
        "INSERT INTO gateway_credentials VALUES ('missing', 'credential', NULL, NULL);",
        "INSERT INTO binding_negotiations VALUES ('missing', 'negotiation', NULL, NULL, NULL);",
        "INSERT INTO device_session_targets VALUES ('missing', 'contest', NULL);",
        "INSERT INTO device_home_targets VALUES ('missing', NULL);",
        "INSERT INTO device_power_targets VALUES ('missing', NULL, NULL);",
    ] {
        rejects(
            &mut connection,
            statement,
            DatabaseErrorKind::ForeignKeyViolation,
        );
    }
    connection.batch_execute(
        "
        INSERT INTO operator_accounts VALUES ('o1', 'operator', 'admin', 'phc', 1);
        INSERT INTO operator_sessions VALUES (x'01', 'o1', 1);
        INSERT INTO seats VALUES ('s1', 'A-01'), ('s2', 'A-02');
        INSERT INTO accounts VALUES ('a1', 'team-1', 1), ('a2', 'team-2', 1);
        INSERT INTO account_mappings VALUES ('s1', 'a1'), ('s2', 'a2');
        INSERT INTO server_vault_records VALUES ('a1', x'01', x'02');
        INSERT INTO devices (device_id, machine_hardware_id, evidence_quality, state, created_at_unix_ms) VALUES ('d1', 'machine-1', 'strong', 'enabled', 1);
        INSERT INTO gateway_credentials VALUES ('d1', 'credential', NULL, NULL);
        INSERT INTO binding_negotiations VALUES ('d1', 'negotiation', NULL, NULL, NULL);
        INSERT INTO device_session_targets VALUES ('d1', 'contest', NULL);
        INSERT INTO device_home_targets VALUES ('d1', NULL);
        INSERT INTO device_power_targets VALUES ('d1', NULL, NULL);
    ",
    )?;
    rejects(
        &mut connection,
        "UPDATE account_mappings SET seat_id = 'missing' WHERE seat_id = 's1';",
        DatabaseErrorKind::ForeignKeyViolation,
    );
    rejects(
        &mut connection,
        "UPDATE account_mappings SET account_id = 'missing' WHERE seat_id = 's1';",
        DatabaseErrorKind::ForeignKeyViolation,
    );
    connection.batch_execute(
        "
        DELETE FROM operator_accounts;
        DELETE FROM accounts WHERE account_id = 'a1';
        DELETE FROM seats WHERE seat_id = 's2';
        DELETE FROM devices;
    ",
    )?;
    for table in [
        "operator_sessions",
        "server_vault_records",
        "account_mappings",
        "gateway_credentials",
        "binding_negotiations",
        "device_session_targets",
        "device_home_targets",
        "device_power_targets",
    ] {
        assert_eq!(
            sql::<BigInt>(&format!("SELECT count(*) FROM {table}"))
                .get_result::<i64>(&mut connection)?,
            0,
            "cascade left rows in {table}"
        );
    }
    Ok(())
}

#[test]
fn keys_and_bindings_restrict_parent_deletion() -> Result<(), TestError> {
    let mut connection = database()?;
    connection.batch_execute(
        "
        INSERT INTO devices (device_id, machine_hardware_id, evidence_quality, state, created_at_unix_ms) VALUES ('d1', 'machine-1', 'strong', 'enabled', 1),
            ('d2', 'machine-2', 'strong', 'enabled', 1);
        INSERT INTO seats VALUES ('s1', 'A-01');
        INSERT INTO device_control_keys VALUES (x'01', 'd1', 'current', 1, NULL);
        INSERT INTO device_bindings VALUES ('b1', 'd2', 's1');
    ",
    )?;
    for statement in [
        "INSERT INTO device_control_keys VALUES (x'02', 'missing', 'current', 1, NULL);",
        "UPDATE device_bindings SET device_id = 'missing';",
        "UPDATE device_bindings SET seat_id = 'missing';",
        "DELETE FROM devices WHERE device_id = 'd1';",
        "DELETE FROM devices WHERE device_id = 'd2';",
        "DELETE FROM seats WHERE seat_id = 's1';",
    ] {
        rejects(
            &mut connection,
            statement,
            DatabaseErrorKind::ForeignKeyViolation,
        );
    }
    connection.batch_execute("DELETE FROM device_bindings; DELETE FROM device_control_keys; DELETE FROM devices; DELETE FROM seats;")?;
    Ok(())
}

#[test]
fn unique_constraints_prevent_duplicate_assignments_and_identifiers() -> Result<(), TestError> {
    let mut connection = database()?;
    connection.batch_execute(
        "
        INSERT INTO operator_accounts VALUES ('o1', 'operator', 'admin', 'phc', 1);
        INSERT INTO seats VALUES ('s1', 'A-01'), ('s2', 'A-02');
        INSERT INTO accounts VALUES ('a1', 'team-1', 1), ('a2', 'team-2', 1);
        INSERT INTO account_mappings VALUES ('s1', 'a1');
        INSERT INTO devices (device_id, machine_hardware_id, evidence_quality, state, created_at_unix_ms) VALUES ('d1', 'machine-1', 'strong', 'enabled', 1),
            ('d2', 'machine-2', 'strong', 'enabled', 1);
        INSERT INTO device_bindings VALUES ('b1', 'd1', 's1');
        INSERT INTO gateway_credentials VALUES ('d1', 'credential', NULL, NULL);
        INSERT INTO binding_negotiations VALUES ('d1', 'negotiation', NULL, NULL, NULL);
    ",
    )?;
    for statement in [
        "INSERT INTO operator_accounts VALUES ('o2', 'operator', 'viewer', 'phc', 1);",
        "INSERT INTO seats VALUES ('s3', 'A-01');",
        "INSERT INTO accounts VALUES ('a3', 'team-1', 1);",
        "INSERT INTO account_mappings VALUES ('s1', 'a2');",
        "INSERT INTO account_mappings VALUES ('s2', 'a1');",
        "INSERT INTO device_bindings VALUES ('b2', 'd1', 's2');",
        "INSERT INTO device_bindings VALUES ('b2', 'd2', 's1');",
        "INSERT INTO gateway_credentials VALUES ('d2', 'credential', NULL, NULL);",
        "INSERT INTO binding_negotiations VALUES ('d2', 'negotiation', NULL, NULL, NULL);",
    ] {
        rejects(
            &mut connection,
            statement,
            DatabaseErrorKind::UniqueViolation,
        );
    }
    Ok(())
}

#[test]
fn partial_unique_indexes_allow_history_but_only_one_current_owner() -> Result<(), TestError> {
    let mut connection = database()?;
    connection
        .batch_execute("INSERT INTO devices (device_id, machine_hardware_id, evidence_quality, state, created_at_unix_ms) VALUES ('d1', 'machine-1', 'strong', 'enabled', 1);")?;
    for state in ["enabled", "disabled"] {
        rejects(
            &mut connection,
            &format!(
                "INSERT INTO devices (device_id, machine_hardware_id, evidence_quality, state, created_at_unix_ms) VALUES ('d2', 'machine-1', 'strong', '{state}', 1);"
            ),
            DatabaseErrorKind::UniqueViolation,
        );
    }
    connection.batch_execute(
        "
        INSERT INTO devices (device_id, machine_hardware_id, evidence_quality, state, created_at_unix_ms) VALUES ('d2', 'machine-1', 'strong', 'revoked', 1);
        UPDATE devices SET state = 'revoked' WHERE device_id = 'd1';
        INSERT INTO devices (device_id, machine_hardware_id, evidence_quality, state, created_at_unix_ms) VALUES ('d3', 'machine-1', 'strong', 'disabled', 1);
        INSERT INTO device_control_keys VALUES (x'01', 'd3', 'current', 1, NULL);
    ",
    )?;
    rejects(
        &mut connection,
        "INSERT INTO device_control_keys VALUES (x'02', 'd3', 'current', 2, NULL);",
        DatabaseErrorKind::UniqueViolation,
    );
    connection.batch_execute("
        INSERT INTO device_control_keys VALUES (x'02', 'd3', 'retired', 1, 2);
        UPDATE device_control_keys SET status = 'retired', retired_at_unix_ms = 2 WHERE public_key = x'01';
        INSERT INTO device_control_keys VALUES (x'03', 'd3', 'current', 2, NULL);
    ")?;
    Ok(())
}

#[test]
fn roster_migration_preserves_deployment_data_and_invalidates_only_old_previews()
-> Result<(), TestError> {
    let mut connection = SqliteConnection::establish(":memory:")?;
    connection.batch_execute("PRAGMA foreign_keys = ON")?;
    connection.run_next_migration(migrations()?)?;
    connection.batch_execute(
        "
        INSERT INTO site_identity VALUES (1, 'fleet');
        INSERT INTO operator_accounts VALUES ('o1', 'admin', 'admin', 'phc', 5);
        INSERT INTO operator_sessions VALUES (x'10', 'o1', 100);
        INSERT INTO seats VALUES ('s1', 'A-01');
        INSERT INTO accounts VALUES ('a1', 'team-1', 7);
        INSERT INTO server_vault_records VALUES ('a1', x'0102', x'0304');
        INSERT INTO account_mappings VALUES ('s1', 'a1');
        INSERT INTO devices (device_id, machine_hardware_id, evidence_quality, state, created_at_unix_ms) VALUES ('d1', 'machine-1', 'strong', 'enabled', 12);
        INSERT INTO device_bindings VALUES ('b1', 'd1', 's1');
        INSERT INTO device_control_keys VALUES (x'05', 'd1', 'current', 13, NULL);
        INSERT INTO gateway_credentials VALUES ('d1', 'g1', x'06', x'07');
        INSERT INTO binding_negotiations VALUES ('d1', 'n1', 8, 'A-01', NULL);
        INSERT INTO device_session_targets VALUES ('d1', 'contest', 9);
        INSERT INTO device_home_targets VALUES ('d1', 10);
        INSERT INTO runtime_config VALUES (1, 'https://judge.example');
        INSERT INTO pending_import_candidate VALUES (1, 'p1', 99, x'08', 1, x'09', x'0a', '{}');
    ",
    )?;
    let checks = [
        "SELECT count(*) FROM site_identity WHERE singleton = 1 AND fleet_namespace_uuid = 'fleet'",
        "SELECT count(*) FROM operator_accounts WHERE operator_id = 'o1' AND password_hash = 'phc' AND credential_revision = 5",
        "SELECT count(*) FROM operator_sessions WHERE session_credential_hash = x'10' AND operator_id = 'o1' AND expires_at_unix_ms = 100",
        "SELECT count(*) FROM accounts a JOIN server_vault_records v USING (account_id) JOIN account_mappings m USING (account_id) JOIN seats s USING (seat_id) WHERE a.account_id = 'a1' AND domjudge_username = 'team-1' AND credential_revision = 7 AND nonce = x'0102' AND ciphertext = x'0304' AND s.seat_id = 's1' AND seat_code = 'A-01'",
        "SELECT count(*) FROM device_bindings b JOIN devices d USING (device_id) WHERE binding_id = 'b1' AND device_id = 'd1' AND seat_id = 's1' AND machine_hardware_id = 'machine-1' AND evidence_quality = 'strong' AND state = 'enabled' AND created_at_unix_ms = 12",
        "SELECT count(*) FROM device_control_keys WHERE public_key = x'05' AND device_id = 'd1' AND status = 'current' AND activated_at_unix_ms = 13 AND retired_at_unix_ms IS NULL",
        "SELECT count(*) FROM gateway_credentials WHERE device_id = 'd1' AND credential_id = 'g1' AND gateway_csr_der = x'06' AND gateway_leaf_der = x'07'",
        "SELECT count(*) FROM binding_negotiations WHERE device_id = 'd1' AND negotiation_id = 'n1' AND submission_epoch = 8 AND seat_code = 'A-01' AND evaluation_error_code IS NULL",
        "SELECT count(*) FROM device_session_targets WHERE device_id = 'd1' AND foreground_target = 'contest' AND terminate_epoch = 9",
        "SELECT count(*) FROM device_home_targets WHERE device_id = 'd1' AND reset_epoch = 10",
        "SELECT count(*) FROM runtime_config WHERE singleton = 1 AND domjudge_origin = 'https://judge.example'",
    ];
    assert_eq!(connection.run_pending_migrations(migrations()?)?.len(), 5);
    for statement in checks {
        assert_eq!(
            sql::<BigInt>(statement).get_result::<i64>(&mut connection)?,
            1
        );
    }
    for table in ["organizations", "teams", "pending_import_candidate"] {
        assert_eq!(
            sql::<BigInt>(&format!("SELECT count(*) FROM {table}"))
                .get_result::<i64>(&mut connection)?,
            0
        );
    }
    Ok(())
}

#[test]
fn roster_relationships_and_organization_sequence_survive_removal() -> Result<(), TestError> {
    let mut connection = database()?;
    connection.batch_execute(
        "
        INSERT INTO accounts VALUES ('a1', 'team-1', 1);
        INSERT INTO organizations VALUES (1, 'University', '', 'University', 'CHN');
        INSERT INTO teams VALUES ('a1', 1, '队伍', 'Team', 'participant');
    ",
    )?;
    for statement in [
        "INSERT INTO teams VALUES ('missing', 1, '', 'Team', 'participant')",
        "UPDATE teams SET organization_id = 2",
        "DELETE FROM organizations WHERE organization_id = 1",
    ] {
        rejects(
            &mut connection,
            statement,
            DatabaseErrorKind::ForeignKeyViolation,
        );
    }
    rejects(
        &mut connection,
        "INSERT INTO organizations VALUES (2, 'University', '', 'University', 'CHN')",
        DatabaseErrorKind::UniqueViolation,
    );
    connection.batch_execute("
        DELETE FROM accounts;
        DELETE FROM organizations;
        INSERT INTO organizations(name_key, name_zh, name_en, country) VALUES ('Next', '', 'Next', 'CHN');
    ")?;
    assert_eq!(
        sql::<BigInt>("SELECT count(*) FROM teams").get_result::<i64>(&mut connection)?,
        0
    );
    assert_eq!(
        sql::<BigInt>("SELECT organization_id FROM organizations")
            .get_result::<i64>(&mut connection)?,
        2
    );
    Ok(())
}
