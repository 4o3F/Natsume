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
    assert_eq!(connection.run_pending_migrations(migrations()?)?.len(), 1);
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
        16
    );
    assert_eq!(connection.revert_all_migrations(migrations()?)?.len(), 1);
    assert_eq!(
        sql::<BigInt>(table_count).get_result::<i64>(&mut connection)?,
        0
    );
    assert_eq!(connection.run_pending_migrations(migrations()?)?.len(), 1);
    assert_eq!(
        sql::<BigInt>(table_count).get_result::<i64>(&mut connection)?,
        16
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
        16
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
        "INSERT INTO device_session_targets VALUES ('missing', 'unlocked', NULL);",
        "INSERT INTO device_home_targets VALUES ('missing', NULL);",
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
        INSERT INTO devices VALUES ('d1', 'machine-1', 'strong', 'enabled', 1);
        INSERT INTO gateway_credentials VALUES ('d1', 'credential', NULL, NULL);
        INSERT INTO binding_negotiations VALUES ('d1', 'negotiation', NULL, NULL, NULL);
        INSERT INTO device_session_targets VALUES ('d1', 'unlocked', NULL);
        INSERT INTO device_home_targets VALUES ('d1', NULL);
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
        INSERT INTO devices VALUES ('d1', 'machine-1', 'strong', 'enabled', 1),
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
        INSERT INTO devices VALUES ('d1', 'machine-1', 'strong', 'enabled', 1),
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
        .batch_execute("INSERT INTO devices VALUES ('d1', 'machine-1', 'strong', 'enabled', 1);")?;
    for state in ["enabled", "disabled"] {
        rejects(
            &mut connection,
            &format!("INSERT INTO devices VALUES ('d2', 'machine-1', 'strong', '{state}', 1);"),
            DatabaseErrorKind::UniqueViolation,
        );
    }
    connection.batch_execute(
        "
        INSERT INTO devices VALUES ('d2', 'machine-1', 'strong', 'revoked', 1);
        UPDATE devices SET state = 'revoked' WHERE device_id = 'd1';
        INSERT INTO devices VALUES ('d3', 'machine-1', 'strong', 'disabled', 1);
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
