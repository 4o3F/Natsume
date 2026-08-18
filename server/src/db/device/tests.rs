use std::{fs, path::PathBuf};

use diesel::{
    Connection, QueryableByName, RunQueryDsl,
    connection::SimpleConnection,
    sql_types::{BigInt, Binary, Text},
    sqlite::SqliteConnection,
};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    application::device::{DeviceError, DeviceLifecycleAction, list_devices},
    audit::{
        self, AuditDetail, AuditEvent, AuditEventId, CorrelationId, DeviceLifecycleAuditResult,
    },
    db::{Database, DatabaseConfig},
};

pub(crate) async fn test_seed_current_facts(
    database: &Database,
    hardware_id_canary: &str,
) -> Result<(), DeviceError> {
    let hardware_id_canary = hardware_id_canary.to_owned();
    database
        .test_write(move |connection| {
            diesel::sql_query(
                "INSERT INTO devices \
                 (device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES \
                 ('01900000-0000-7000-8000-000000000002', \
                  'machine-hardware-b', 'medium', 'disabled'), \
                 ('01900000-0000-7000-8000-000000000001', ?, 'strong', 'enrolled')",
            )
            .bind::<Text, _>(&hardware_id_canary)
            .execute(connection)
            .map(|_| ())
            .map_err(|_| DeviceError::PersistenceFailed)
        })
        .await
        .map_err(|_| DeviceError::PersistenceFailed)?
}

pub(crate) async fn test_seed_lifecycle_device(
    database: &Database,
    device_id: &str,
    state: &str,
    with_token: bool,
    certificate_status: &str,
) -> Result<(), DeviceError> {
    let device_id = device_id.to_owned();
    let state = state.to_owned();
    let certificate_status = certificate_status.to_owned();
    database
    .test_write(move |connection| {
        let enrollment_id = format!("enrollment-{device_id}");
        let seat_id = format!("seat-{device_id}");
        let hardware_id = format!("hardware-secret-{device_id}");
        diesel::sql_query("INSERT INTO seats (seat_id, seat_code) VALUES (?, ?)")
            .bind::<Text, _>(&seat_id)
            .bind::<Text, _>(format!("code-{device_id}"))
            .execute(connection)
            .map_err(|_| DeviceError::PersistenceFailed)?;
        diesel::sql_query(
            "INSERT INTO devices \
             (device_pk, machine_hardware_id, hardware_identity_quality, state) \
             VALUES (?, ?, 'strong', ?)",
        )
        .bind::<Text, _>(&device_id)
        .bind::<Text, _>(&hardware_id)
        .bind::<Text, _>(&state)
        .execute(connection)
        .map_err(|_| DeviceError::PersistenceFailed)?;
        diesel::sql_query(
            "INSERT INTO enrollment_requests \
             (enrollment_request_id, machine_hardware_id, hardware_identity_quality, \
              gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, source_ip, \
              state, created_at) \
             VALUES (?, ?, 'strong', ?, ?, 'test-client', 1, '192.0.2.1', 'pending', \
                     '2026-08-08T00:00:00.000Z')",
        )
        .bind::<Text, _>(&enrollment_id)
        .bind::<Text, _>(&hardware_id)
        .bind::<Binary, _>(b"certificate-material-secret-canary".as_slice())
        .bind::<Binary, _>(b"spki-hash-secret-canary-12345678".as_slice())
        .execute(connection)
        .map_err(|_| DeviceError::PersistenceFailed)?;
        if with_token {
            let mut token_hash = *b"token-hash-secret-canary-1234567";
            let unique_byte = device_id
                .as_bytes()
                .last()
                .copied()
                .ok_or(DeviceError::InvalidPersistedFacts)?;
            token_hash[31] = unique_byte;
            diesel::sql_query(
                "INSERT INTO device_tokens (device_pk, enrollment_request_id, token_hash) \
                 VALUES (?, ?, ?)",
            )
            .bind::<Text, _>(&device_id)
            .bind::<Text, _>(&enrollment_id)
            .bind::<Binary, _>(token_hash.as_slice())
            .execute(connection)
            .map_err(|_| DeviceError::PersistenceFailed)?;
        }
        diesel::sql_query(
            "INSERT INTO gateway_certificates \
             (certificate_id, device_pk, enrollment_request_id, serial, spki_sha256, not_after, status) \
             VALUES (?, ?, ?, ?, ?, '2027-08-08T00:00:00.000Z', ?)",
        )
        .bind::<Text, _>(format!("certificate-{device_id}"))
        .bind::<Text, _>(&device_id)
        .bind::<Text, _>(&enrollment_id)
        .bind::<Text, _>(format!("certificate-serial-secret-{device_id}"))
        .bind::<Binary, _>(b"spki-secret-canary-1234567890123".as_slice())
        .bind::<Text, _>(&certificate_status)
        .execute(connection)
        .map_err(|_| DeviceError::PersistenceFailed)?;
        diesel::sql_query(
            "INSERT INTO device_bindings (seat_id, device_pk, binding_revision) \
             VALUES (?, ?, 43)",
        )
        .bind::<Text, _>(&seat_id)
        .bind::<Text, _>(&device_id)
        .execute(connection)
        .map_err(|_| DeviceError::PersistenceFailed)?;
        diesel::sql_query(
            "UPDATE revision_counters SET configuration_revision = 41, \
             binding_revision = 43 WHERE singleton = 1",
        )
        .execute(connection)
        .map(|_| ())
        .map_err(|_| DeviceError::PersistenceFailed)
    })
    .await
    .map_err(|_| DeviceError::PersistenceFailed)?
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TestLifecycleSnapshot {
    pub(crate) state: String,
    pub(crate) token_count: i64,
    pub(crate) certificate_statuses: Vec<String>,
    pub(crate) binding_revision: i64,
    pub(crate) configuration_revision: i64,
    pub(crate) global_binding_revision: i64,
    pub(crate) command_count: i64,
}

pub(crate) async fn test_lifecycle_snapshot(
    database: &Database,
    device_id: &str,
) -> Result<TestLifecycleSnapshot, DeviceError> {
    let device_id = device_id.to_owned();
    database
        .test_read(move |connection| {
            let row = diesel::sql_query(
                "SELECT devices.state AS state, \
             (SELECT COUNT(*) FROM device_tokens WHERE device_pk = devices.device_pk) \
                 AS token_count, \
             device_bindings.binding_revision AS binding_revision, \
             revision_counters.configuration_revision AS configuration_revision, \
             revision_counters.binding_revision AS global_binding_revision \
             FROM devices \
             JOIN device_bindings ON device_bindings.device_pk = devices.device_pk \
             JOIN revision_counters ON revision_counters.singleton = 1 \
             WHERE devices.device_pk = ?",
            )
            .bind::<Text, _>(&device_id)
            .get_result::<TestLifecycleSnapshotRow>(connection)
            .map_err(|_| DeviceError::PersistenceFailed)?;
            let certificate_statuses = diesel::sql_query(
                "SELECT status AS value FROM gateway_certificates WHERE device_pk = ? \
             ORDER BY certificate_id",
            )
            .bind::<Text, _>(&device_id)
            .load::<TestTextRow>(connection)
            .map_err(|_| DeviceError::PersistenceFailed)?
            .into_iter()
            .map(|status| status.value)
            .collect();
            let command_count =
                diesel::sql_query("SELECT COUNT(*) AS value FROM commands WHERE device_pk = ?")
                    .bind::<Text, _>(&device_id)
                    .get_result::<TestIntegerRow>(connection)
                    .map_err(|_| DeviceError::PersistenceFailed)?
                    .value;
            Ok(TestLifecycleSnapshot {
                state: row.state,
                token_count: row.token_count,
                certificate_statuses,
                binding_revision: row.binding_revision,
                configuration_revision: row.configuration_revision,
                global_binding_revision: row.global_binding_revision,
                command_count,
            })
        })
        .await
        .map_err(|_| DeviceError::PersistenceFailed)?
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TestLifecycleAudit {
    pub(crate) actor: String,
    pub(crate) action: String,
    pub(crate) resource_type: String,
    pub(crate) resource_id: String,
    pub(crate) result: String,
    pub(crate) reason: String,
    pub(crate) detail: String,
    pub(crate) complete_row: String,
}

pub(crate) async fn test_latest_lifecycle_audit(
    database: &Database,
    device_id: &str,
) -> Result<TestLifecycleAudit, DeviceError> {
    let device_id = device_id.to_owned();
    database
        .test_read(move |connection| {
            let row = diesel::sql_query(
                "SELECT actor, action_kind AS action, resource_type, resource_id, result, \
             reason_code AS reason, redacted_detail_json AS detail, \
             audit_event_id || occurred_at || actor || action_kind || resource_type || \
             resource_id || result || COALESCE(reason_code, '') || correlation_id || \
             COALESCE(group_correlation_id, '') || redacted_detail_json AS complete_row \
             FROM audit_events WHERE resource_type = 'device' AND resource_id = ? \
             ORDER BY rowid DESC LIMIT 1",
            )
            .bind::<Text, _>(&device_id)
            .get_result::<TestLifecycleAuditRow>(connection)
            .map_err(|_| DeviceError::PersistenceFailed)?;
            Ok(TestLifecycleAudit {
                actor: row.actor,
                action: row.action,
                resource_type: row.resource_type,
                resource_id: row.resource_id,
                result: row.result,
                reason: row.reason,
                detail: row.detail,
                complete_row: row.complete_row,
            })
        })
        .await
        .map_err(|_| DeviceError::PersistenceFailed)?
}

pub(crate) async fn test_lifecycle_audit_count(
    database: &Database,
    device_id: &str,
) -> Result<i64, DeviceError> {
    let device_id = device_id.to_owned();
    database
        .test_read(move |connection| {
            diesel::sql_query(
                "SELECT COUNT(*) AS value FROM audit_events \
             WHERE resource_type = 'device' AND resource_id = ?",
            )
            .bind::<Text, _>(&device_id)
            .get_result::<TestIntegerRow>(connection)
            .map(|row| row.value)
            .map_err(|_| DeviceError::PersistenceFailed)
        })
        .await
        .map_err(|_| DeviceError::PersistenceFailed)?
}

pub(crate) async fn test_reserve_audit_id(
    database: &Database,
    audit_event_id: AuditEventId,
) -> Result<(), DeviceError> {
    database
        .test_write(move |connection| {
            let event = AuditEvent::device_lifecycle(
                audit_event_id,
                CorrelationId::from_uuid(Uuid::now_v7()),
                "reserved-device".to_owned(),
                DeviceLifecycleAction::Disable,
                DeviceLifecycleAuditResult::Noop,
                AuditDetail::DeviceLifecycle {
                    resulting_state: "disabled",
                    removed_token_count: 0,
                    revoked_certificate_count: 0,
                },
            );
            audit::insert_diesel(connection, &event).map_err(|_| DeviceError::PersistenceFailed)
        })
        .await
        .map_err(|_| DeviceError::PersistenceFailed)?
}

#[derive(QueryableByName)]
struct TestTextRow {
    #[diesel(sql_type = Text)]
    value: String,
}

#[derive(QueryableByName)]
struct TestIntegerRow {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

#[derive(QueryableByName)]
struct TestLifecycleSnapshotRow {
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = BigInt)]
    token_count: i64,
    #[diesel(sql_type = BigInt)]
    binding_revision: i64,
    #[diesel(sql_type = BigInt)]
    configuration_revision: i64,
    #[diesel(sql_type = BigInt)]
    global_binding_revision: i64,
}

#[derive(QueryableByName)]
struct TestLifecycleAuditRow {
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Text)]
    action: String,
    #[diesel(sql_type = Text)]
    resource_type: String,
    #[diesel(sql_type = Text)]
    resource_id: String,
    #[diesel(sql_type = Text)]
    result: String,
    #[diesel(sql_type = Text)]
    reason: String,
    #[diesel(sql_type = Text)]
    detail: String,
    #[diesel(sql_type = Text)]
    complete_row: String,
}
#[tokio::test]
async fn persisted_device_vocabulary_outside_the_frozen_set_fails_closed() -> Result<(), TestFailure>
{
    let fixture = TestDatabase::new().await?;
    test_seed_lifecycle_device(
        &fixture.database,
        "01900000-0000-7000-8000-000000000101",
        "enrolled",
        true,
        "active",
    )
    .await
    .map_err(|_| TestFailure::FixtureFailed)?;
    if list_devices(&fixture.database).await.is_err() {
        return Err(TestFailure::FrozenVocabularyWasRejected);
    }

    for statement in [
        "UPDATE devices SET state = 'quarantined'",
        "UPDATE devices SET state = 'enrolled', \
         hardware_identity_quality = 'excellent'",
    ] {
        poison_device_vocabulary(&fixture.path, statement)?;
        if !matches!(
            list_devices(&fixture.database).await,
            Err(DeviceError::InvalidPersistedFacts)
        ) {
            return Err(TestFailure::UnfrozenVocabularyWasAccepted);
        }
    }
    Ok(())
}

fn poison_device_vocabulary(
    database_path: &std::path::Path,
    statement: &str,
) -> Result<(), TestFailure> {
    let path = database_path.to_str().ok_or(TestFailure::FixtureFailed)?;
    let mut connection =
        SqliteConnection::establish(path).map_err(|_| TestFailure::FixtureFailed)?;
    connection
        .batch_execute("PRAGMA ignore_check_constraints = ON;")
        .map_err(|_| TestFailure::FixtureFailed)?;
    diesel::sql_query(statement)
        .execute(&mut connection)
        .map(|_| ())
        .map_err(|_| TestFailure::FixtureFailed)
}

struct TestDatabase {
    database: Database,
    path: PathBuf,
}

impl TestDatabase {
    async fn new() -> Result<Self, TestFailure> {
        let path = std::env::temp_dir().join(format!(
            "natsume-device-query-test-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .map_err(|_| TestFailure::FixtureFailed)?;
        Ok(Self { database, path })
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _database_result = fs::remove_file(&self.path);
        let _wal_result = fs::remove_file(format!("{}-wal", self.path.display()));
        let _shm_result = fs::remove_file(format!("{}-shm", self.path.display()));
    }
}

#[derive(Debug, Snafu)]
enum TestFailure {
    #[snafu(display("the Device query fixture failed"))]
    FixtureFailed,
    #[snafu(display("the frozen persisted Device vocabulary was rejected"))]
    FrozenVocabularyWasRejected,
    #[snafu(display("an unfrozen persisted Device vocabulary value was accepted"))]
    UnfrozenVocabularyWasAccepted,
}
