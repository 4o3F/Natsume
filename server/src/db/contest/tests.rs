use diesel::{
    Connection, QueryableByName, RunQueryDsl,
    sql_types::{BigInt, Text},
    sqlite::SqliteConnection,
};

use crate::{application::contest::ContestError, db::Database, vault::VaultRecordType};

pub(crate) struct TestObserver {
    connection: SqliteConnection,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TestPersistenceSnapshot {
    sessions: i64,
    audits: i64,
    data_version: i64,
    expiries: Vec<String>,
}

pub(crate) fn test_observer(database_path: &std::path::Path) -> Result<TestObserver, ContestError> {
    let path = database_path
        .to_str()
        .ok_or(ContestError::PersistenceFailed)?;
    let connection =
        SqliteConnection::establish(path).map_err(|_| ContestError::PersistenceFailed)?;
    Ok(TestObserver { connection })
}

pub(crate) async fn test_snapshot(
    database: &Database,
    observer: &mut TestObserver,
) -> Result<TestPersistenceSnapshot, ContestError> {
    let (sessions, audits, expiries) = database
        .test_read(|connection| {
            let counts = diesel::sql_query(
                "SELECT (SELECT COUNT(*) FROM operator_sessions) AS sessions, \
                 (SELECT COUNT(*) FROM audit_events) AS audits",
            )
            .get_result::<TestPersistenceCountsRow>(connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
            let expiries = diesel::sql_query(
                "SELECT expires_at AS value FROM operator_sessions \
                 ORDER BY session_credential_hash",
            )
            .load::<TestTextRow>(connection)
            .map_err(|_| ContestError::PersistenceFailed)?
            .into_iter()
            .map(|row| row.value)
            .collect();
            Ok::<_, ContestError>((counts.sessions, counts.audits, expiries))
        })
        .await
        .map_err(|_| ContestError::PersistenceFailed)??;
    let data_version = diesel::dsl::sql::<BigInt>("PRAGMA data_version")
        .get_result(&mut observer.connection)
        .map_err(|_| ContestError::PersistenceFailed)?;
    Ok(TestPersistenceSnapshot {
        sessions,
        audits,
        data_version,
        expiries,
    })
}

pub(crate) async fn test_expire_all_sessions(database: &Database) -> Result<(), ContestError> {
    database
        .test_write(|connection| {
            diesel::sql_query(
                "UPDATE operator_sessions \
                 SET expires_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 second')",
            )
            .execute(connection)
            .map(|_| ())
            .map_err(|_| ContestError::PersistenceFailed)
        })
        .await
        .map_err(|_| ContestError::PersistenceFailed)?
}

pub(crate) async fn test_seed_current_facts(
    database: &Database,
    vault_pointer_canary: &str,
) -> Result<(), ContestError> {
    let vault_pointer_canary = vault_pointer_canary.to_owned();
    let record_type = VaultRecordType::AccountCredential.as_str();
    database
        .test_write(move |connection| {
            diesel::sql_query(
                "INSERT INTO server_vault_records \
                 (vault_record_id, record_type, subject_id, nonce, ciphertext) VALUES \
                 (?, ?, 'account-a', x'01', x'02'), \
                 ('vault-record-b', ?, 'account-b', x'03', x'04')",
            )
            .bind::<Text, _>(&vault_pointer_canary)
            .bind::<Text, _>(record_type)
            .bind::<Text, _>(record_type)
            .execute(connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
            diesel::sql_query(
                "INSERT INTO seats (seat_id, seat_code) VALUES \
                 ('seat-b', 'B-02'), ('seat-a', 'A-01')",
            )
            .execute(connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
            diesel::sql_query(
                "INSERT INTO accounts \
                 (account_id, domjudge_username, credential_vault_record_id, \
                  credential_revision) VALUES \
                 ('account-b', 'team-beta', 'vault-record-b', 7), \
                 ('account-a', 'team-alpha', ?, 3)",
            )
            .bind::<Text, _>(&vault_pointer_canary)
            .execute(connection)
            .map_err(|_| ContestError::PersistenceFailed)?;
            diesel::sql_query(
                "INSERT INTO device_bindings (seat_id, device_pk, binding_revision) VALUES \
                 ('seat-b', '01900000-0000-7000-8000-000000000002', 11), \
                 ('seat-a', '01900000-0000-7000-8000-000000000001', 11)",
            )
            .execute(connection)
            .map(|_| ())
            .map_err(|_| ContestError::PersistenceFailed)
        })
        .await
        .map_err(|_| ContestError::PersistenceFailed)?
}

#[derive(QueryableByName)]
struct TestPersistenceCountsRow {
    #[diesel(sql_type = BigInt)]
    sessions: i64,
    #[diesel(sql_type = BigInt)]
    audits: i64,
}

#[derive(QueryableByName)]
struct TestTextRow {
    #[diesel(sql_type = Text)]
    value: String,
}
