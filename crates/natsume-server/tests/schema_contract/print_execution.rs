use diesel::{
    Connection as _, RunQueryDsl as _, connection::SimpleConnection as _, dsl::sql,
    sql_types::BigInt, sqlite::SqliteConnection,
};
use diesel_migrations::MigrationHarness as _;

use super::{migrations, rejects};
use diesel::result::DatabaseErrorKind;

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("print execution migration failed: {error:?}"))
}

#[test]
fn one_print_migration_creates_and_removes_all_print_tables_together() {
    let mut connection = must(SqliteConnection::establish(":memory:"));
    must(connection.batch_execute("PRAGMA foreign_keys=ON"));
    for _ in 0..6 {
        must(connection.run_next_migration(must(migrations())));
    }
    must(connection.batch_execute("INSERT INTO accounts VALUES ('a','team',1)"));
    let print_tables =
        "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name LIKE 'print_%'";
    assert_eq!(
        must(sql::<BigInt>(print_tables).get_result::<i64>(&mut connection)),
        0
    );
    assert_eq!(
        must(connection.run_pending_migrations(must(migrations()))).len(),
        1
    );
    assert_eq!(
        must(sql::<BigInt>(print_tables).get_result::<i64>(&mut connection)),
        7
    );
    assert!(must(connection.run_pending_migrations(must(migrations()))).is_empty());
    must(connection.revert_last_migration(must(migrations())));
    assert_eq!(
        must(sql::<BigInt>(print_tables).get_result::<i64>(&mut connection)),
        0
    );
    assert_eq!(
        must(
            sql::<BigInt>(
                "SELECT count(*) FROM accounts WHERE account_id='a' AND domjudge_username='team'"
            )
            .get_result::<i64>(&mut connection)
        ),
        1
    );
    assert_eq!(
        must(connection.run_pending_migrations(must(migrations()))).len(),
        1
    );
    assert_eq!(
        must(sql::<BigInt>(print_tables).get_result::<i64>(&mut connection)),
        7
    );
}

#[test]
fn execution_schema_enforces_active_ownership_receipts_and_attempt_bounds() {
    let mut connection = must(super::database());
    must(connection.batch_execute("INSERT INTO print_sources VALUES ('s','judge',x'01',0); INSERT INTO print_requests VALUES ('s','r','a','team','Team',NULL,'main.cpp','hash',NULL); INSERT INTO print_documents (source_id,request_id,sha256,bytes,pages,ahead_tasks,ahead_pages) VALUES ('s','r','pdf',12,3,0,0); INSERT INTO print_printers VALUES ('p','Printer',NULL,NULL,'client',x'02',1); INSERT INTO print_jobs VALUES (1,'active',1,0,1,NULL); INSERT INTO print_attempts (attempt_id,ready_sequence,printer_id,client_id,attempt_number,queue_name,claimed_at_unix_ms,prepare_expires_at_unix_ms,lease_expires_at_unix_ms,last_contact_at_unix_ms) VALUES ('a',1,'p','client',1,'queue',0,120000,60000,0); INSERT INTO print_claims VALUES ('client','claim','queue','a');"));
    for statement in [
        "INSERT INTO print_claims VALUES ('client','claim','queue',NULL)",
        "INSERT INTO print_attempts (attempt_id,ready_sequence,printer_id,client_id,attempt_number,queue_name,claimed_at_unix_ms,prepare_expires_at_unix_ms,lease_expires_at_unix_ms,last_contact_at_unix_ms) VALUES ('b',1,'p','client',2,'queue',0,120000,60000,0)",
    ] {
        rejects(
            &mut connection,
            statement,
            DatabaseErrorKind::UniqueViolation,
        );
    }
    for statement in [
        "UPDATE print_jobs SET attempts_used=4",
        "UPDATE print_jobs SET state='cancelled'",
        "UPDATE print_attempts SET job_id=0",
        "UPDATE print_attempts SET job_id=4294967296",
        "UPDATE print_attempts SET outcome='printed',finished_at_unix_ms=1",
        "UPDATE print_attempts SET outcome='failed'",
    ] {
        assert!(connection.batch_execute(statement).is_err(), "{statement}");
    }
    rejects(
        &mut connection,
        "DELETE FROM print_printers",
        DatabaseErrorKind::ForeignKeyViolation,
    );
    must(connection.batch_execute("UPDATE print_attempts SET outcome='unknown',finished_at_unix_ms=60000; INSERT INTO print_attempts (attempt_id,ready_sequence,printer_id,client_id,attempt_number,queue_name,claimed_at_unix_ms,prepare_expires_at_unix_ms,lease_expires_at_unix_ms,last_contact_at_unix_ms) VALUES ('b',1,'p','client',2,'queue',90000,210000,150000,90000)"));
}
